//! One bounded Long64 CPL0 paged #PF delivery through a same-CPL IST=0 gate.

use crate::{
    AdUpdate, GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineBus, PageFaultReason, ProbeConfigError, ProbeError, ProbeOutcome, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::execute::step_paged_exception_stack_discard as cpu_step_paged_exception_stack_discard;
use od_isa::{
    CpuActivity, LongModeState, OperandSize, PageFaultDeliveryResult, PagedCpl0Cr0WpMovV1,
    PagedCpl0PageTableRepairMovV1, PagedProbeResult, deliver_page_fault,
    execute::PagedCpl0ExceptionStackDiscardV1,
    interrupt::deliver_double_fault,
    iretq::IretqResult,
    memory::{
        AccessContext, AccessKind, DoubleFaultRecord, GuestMemory, MemoryError, PageFaultRecord,
    },
    step_paged_cr0_wp_mov as cpu_step_paged_cr0_wp_mov,
    step_paged_cr0_wp_page_table_repair_mov as cpu_step_paged_cr0_wp_page_table_repair_mov,
};
use std::convert::Infallible;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedExceptionDeliveryError {
    Configuration(ProbeConfigError),
}

/// Bounded #PF delivery with at most one #DF attempt. Effects from failed
/// attempts remain committed and are reported in execution order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedExceptionDeliveryReport {
    pub original_fault: PageFaultRecord,
    /// Result of the initial vector-14 delivery attempt.
    pub delivery: PageFaultDeliveryResult,
    pub outcome: PagedExceptionDeliveryOutcome,
    pub secondary_page_fault: Option<PageFaultRecord>,
    pub double_fault: Option<DoubleFaultRecord>,
    pub double_fault_delivery: Option<PageFaultDeliveryResult>,
    pub double_fault_secondary_page_fault: Option<PageFaultRecord>,
    pub access_error: Option<TrackedAccessError>,
    pub double_fault_access_error: Option<TrackedAccessError>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagedExceptionDeliveryOutcome {
    PageFaultDelivered,
    DoubleFaultDelivered,
    Shutdown,
    Undelivered,
}

/// Result of a delivery followed by the handler's first profiled MOV. A
/// handler fetch/data fault is reported in `handler_step` after a successful
/// `delivery`; it is not conflated with a fault while constructing the frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedExceptionHandlerEntryReport {
    pub delivery: PagedExceptionDeliveryReport,
    pub handler_step: Option<Result<PagedCr0WpMovStepReport, PagedExceptionDeliveryError>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedCr0WpMovStepReport {
    pub cpu: PagedProbeResult<Infallible>,
    pub page_fault: Option<PageFaultRecord>,
    pub access_error: Option<TrackedAccessError>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedExceptionStackDiscardReport {
    pub cpu: PagedProbeResult<Infallible>,
    pub page_fault: Option<PageFaultRecord>,
    pub access_error: Option<TrackedAccessError>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedIretqStepReport {
    pub cpu: IretqResult,
    pub rflags_defined: u64,
    pub page_fault: Option<PageFaultRecord>,
    pub access_error: Option<TrackedAccessError>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

/// Execute one exact paged qword MOV through the CPU-owned CR0.WP profile.
/// This supplies the real instruction step that can raise the initial #PF.
pub fn step_paged_cr0_wp_mov(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedCr0WpMovStepReport, PagedExceptionDeliveryError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedExceptionDeliveryError::Configuration)?;
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: state.rip,
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let cpu = cpu_step_paged_cr0_wp_mov(
        PagedCpl0Cr0WpMovV1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    );
    if let Some(record) = &memory.secondary_page_fault {
        state.cr2 = Some(record.cr2_value());
    }
    Ok(PagedCr0WpMovStepReport {
        cpu,
        page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    })
}

/// Execute the bounded guest page-table repair store through tracked RAM.
pub fn step_paged_cr0_wp_page_table_repair_mov(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedCr0WpMovStepReport, PagedExceptionDeliveryError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedExceptionDeliveryError::Configuration)?;
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: state.rip,
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let cpu = cpu_step_paged_cr0_wp_page_table_repair_mov(
        PagedCpl0PageTableRepairMovV1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    );
    if let Some(record) = &memory.secondary_page_fault {
        state.cr2 = Some(record.cr2_value());
    }
    Ok(PagedCr0WpMovStepReport {
        cpu,
        page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    })
}

/// Execute the guest LEA that discards the #PF error-code stack slot.
pub fn step_paged_exception_stack_discard(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedExceptionStackDiscardReport, PagedExceptionDeliveryError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedExceptionDeliveryError::Configuration)?;
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: state.rip,
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let cpu = cpu_step_paged_exception_stack_discard(
        PagedCpl0ExceptionStackDiscardV1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    );
    if let Some(record) = &memory.secondary_page_fault {
        state.cr2 = Some(record.cr2_value());
    }
    Ok(PagedExceptionStackDiscardReport {
        cpu,
        page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    })
}

/// Execute one same-CPL paged IRETQ after the guest has discarded its error code.
pub fn step_paged_iretq(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedIretqStepReport, PagedExceptionDeliveryError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedExceptionDeliveryError::Configuration)?;
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: state.rip,
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let mut defined = rflags_defined;
    let cpu = od_isa::iretq::step_paged_iretq(state, &mut memory, context, &mut defined, efer);
    if let Some(record) = &memory.secondary_page_fault {
        state.cr2 = Some(record.cr2_value());
    }
    Ok(PagedIretqStepReport {
        cpu,
        rflags_defined: defined,
        page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    })
}

/// The committed return and subsequent user instruction have separate outcomes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedUserReturnReport {
    pub iretq: PagedIretqStepReport,
    pub mov: Option<PagedCr0WpMovStepReport>,
}

pub fn step_paged_iretq_and_user_load(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedUserReturnReport, PagedExceptionDeliveryError> {
    use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};
    let cr3 = state.cr3.ok_or(PagedExceptionDeliveryError::Configuration(
        ProbeConfigError::MissingCpuRegister { register: "CR3" },
    ))?;
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, 0xd00)
        .map_err(PagedExceptionDeliveryError::Configuration)?;
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: state.rip,
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let mut defined = rflags_defined;
    let cpu = od_isa::iretq::step_paged_iretq_to_cpl3(
        od_isa::iretq::PagedCpl0ToCpl3V1,
        od_isa::cpuid::CpuidProfile::DiagnosticV1,
        state,
        &mut memory,
        context,
        &mut defined,
        0xd00,
    );
    if let Some(record) = &memory.secondary_page_fault {
        state.cr2 = Some(record.cr2_value());
    }
    let iretq = PagedIretqStepReport {
        cpu,
        rflags_defined: defined,
        page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    };
    if iretq.cpu != IretqResult::Retired {
        return Ok(PagedUserReturnReport { iretq, mov: None });
    }
    let valid_cs = state.cs_selector.is_some_and(|s| s & !7 != 0 && s & 7 == 3)
        && state.cs_cache.is_some_and(|c| {
            c.access == 0xfb && c.flags == 0xa && c.base == 0 && c.limit == u32::MAX
        });
    if !valid_cs {
        return Ok(PagedUserReturnReport { iretq, mov: None });
    }
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: state.rip,
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let context = AccessContext {
        privilege: PrivilegeLevel::Ring3,
        ..context
    };
    let cpu = od_isa::execute::step_paged_cpl3_load(state, &mut memory, context, 0xd00, defined);
    if let Some(record) = &memory.secondary_page_fault {
        state.cr2 = Some(record.cr2_value());
    }
    let mov = PagedCr0WpMovStepReport {
        cpu,
        page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    };
    Ok(PagedUserReturnReport {
        iretq,
        mov: Some(mov),
    })
}

pub fn deliver_paged_page_fault(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    original_fault: &PageFaultRecord,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedExceptionDeliveryReport, PagedExceptionDeliveryError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedExceptionDeliveryError::Configuration)?;
    let mut memory = PagedExceptionMemory {
        bus,
        paging,
        instruction_rip: original_fault.saved_rip(),
        committed_write: &mut committed_write,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
        descriptor_accessed_target: None,
    };
    let delivery = deliver_page_fault(state, &mut memory, original_fault, context);
    let secondary_page_fault = memory.secondary_page_fault.take();
    let access_error = memory.access_error.take();
    let mut double_fault = None;
    let mut double_fault_delivery = None;
    let mut double_fault_secondary_page_fault = None;
    let mut double_fault_access_error = None;
    let mut outcome = if matches!(delivery, PageFaultDeliveryResult::Delivered { .. }) {
        PagedExceptionDeliveryOutcome::PageFaultDelivered
    } else {
        PagedExceptionDeliveryOutcome::Undelivered
    };

    if is_architectural_page_fault_abort(&delivery) {
        if let Some(secondary) = secondary_page_fault.as_ref() {
            let record = DoubleFaultRecord::new(original_fault.clone(), secondary.clone());
            memory.secondary_page_fault = None;
            memory.access_error = None;
            let result = deliver_double_fault(state, &mut memory, &record, context);
            let shutdown_fault =
                is_architectural_page_fault_abort(&result) && memory.secondary_page_fault.is_some();
            double_fault_secondary_page_fault = memory.secondary_page_fault.take();
            double_fault_access_error = memory.access_error.take();
            outcome = if matches!(result, PageFaultDeliveryResult::Delivered { .. }) {
                PagedExceptionDeliveryOutcome::DoubleFaultDelivered
            } else if shutdown_fault {
                state.activity = Some(CpuActivity::Shutdown);
                PagedExceptionDeliveryOutcome::Shutdown
            } else {
                PagedExceptionDeliveryOutcome::Undelivered
            };
            double_fault = Some(record);
            double_fault_delivery = Some(result);
        }
    }
    Ok(PagedExceptionDeliveryReport {
        original_fault: original_fault.clone(),
        delivery,
        outcome,
        secondary_page_fault,
        double_fault,
        double_fault_delivery,
        double_fault_secondary_page_fault,
        access_error,
        double_fault_access_error,
        ad_updates: memory.ad_updates,
        committed_writes: memory.committed_writes,
    })
}

pub fn deliver_and_step_paged_pf_handler(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    original_fault: &PageFaultRecord,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedExceptionHandlerEntryReport, PagedExceptionDeliveryError> {
    let delivery = deliver_paged_page_fault(
        state,
        bus,
        cr3,
        efer,
        context,
        original_fault,
        &mut committed_write,
    )?;
    let handler_step = if matches!(
        delivery.outcome,
        PagedExceptionDeliveryOutcome::PageFaultDelivered
            | PagedExceptionDeliveryOutcome::DoubleFaultDelivered
    ) {
        let step = step_paged_cr0_wp_mov(
            state,
            bus,
            cr3,
            efer,
            context,
            rflags_defined,
            &mut committed_write,
        );
        Some(step)
    } else {
        None
    };
    Ok(PagedExceptionHandlerEntryReport {
        delivery,
        handler_step,
    })
}

fn is_architectural_page_fault_abort(result: &PageFaultDeliveryResult) -> bool {
    matches!(
        result,
        PageFaultDeliveryResult::DeliveryAborted {
            cause: MemoryError::Architectural(fault),
            ..
        } if fault.vector == 14
    )
}

struct PagedExceptionMemory<'a> {
    bus: &'a mut MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    secondary_page_fault: Option<PageFaultRecord>,
    access_error: Option<TrackedAccessError>,
    ad_updates: Vec<AdUpdate>,
    committed_writes: Vec<GuestPhysicalRange>,
    descriptor_accessed_target: Option<GuestPhysicalAddress>,
}

impl PagedExceptionMemory<'_> {
    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context,
            operation,
        };
        let Self {
            bus,
            paging,
            committed_write,
            committed_writes,
            ad_updates,
            ..
        } = self;
        let report = bus.access_tracked_ram(paging, request, |range| {
            committed_writes.push(range);
            committed_write(range);
        });
        ad_updates.extend(
            report.ad_updates[..report.ad_update_count]
                .iter()
                .flatten()
                .copied(),
        );
        match report.outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => Err(self.map_error(error)),
        }
    }

    fn map_error(&mut self, error: TrackedAccessError) -> MemoryError {
        match error {
            TrackedAccessError::PageFault { record, .. } => {
                self.secondary_page_fault = Some(record.clone());
                MemoryError::Architectural(record.fault().clone())
            }
            error => {
                let message = format!("tracked paged exception access failed: {error:?}");
                self.access_error = Some(error);
                MemoryError::Backend(message)
            }
        }
    }

    fn read_chunk(
        &mut self,
        address: u64,
        context: AccessContext,
        width: AccessWidth,
    ) -> Result<u64, MemoryError> {
        match self.access(address, context, RamAccessOperation::Read { width })? {
            TrackedAccessOutcome::Read(value) => Ok(value),
            _ => Err(MemoryError::Backend(
                "tracked read returned a non-read outcome".into(),
            )),
        }
    }

    fn descriptor_mapping_matches_preflight(&self, address: u64, context: AccessContext) -> bool {
        let Some(expected) = self.descriptor_accessed_target else {
            return false;
        };
        let report = self.bus.probe_tracked_ram_access(
            &self.paging,
            GuestLinearAddress::new(address),
            AccessKind::Read,
            context,
        );
        matches!(report.outcome, Ok(ProbeOutcome::Translated(t)) if t.physical == expected)
    }
}

use crate::AccessWidth;

impl GuestMemory for PagedExceptionMemory<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        match self.access(address, context, RamAccessOperation::Fetch)? {
            TrackedAccessOutcome::Fetched(byte) => Ok(byte),
            _ => Err(MemoryError::Backend(
                "tracked fetch returned a non-fetch outcome".into(),
            )),
        }
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        let mut scratch = Vec::with_capacity(destination.len());
        let mut offset = 0usize;
        while offset < destination.len() {
            let current =
                address
                    .checked_add(offset as u64)
                    .ok_or(MemoryError::AddressOverflow {
                        address,
                        length: destination.len(),
                    })?;
            let page_remaining = 0x1000 - (current & 0xfff);
            let remaining = (destination.len() - offset) as u64;
            let mut bytes = remaining.min(page_remaining).min(8) as usize;
            while !matches!(bytes, 1 | 2 | 4 | 8) {
                bytes -= 1;
            }
            let width = access_width(bytes)?;
            let value = self.read_chunk(current, context, width)?;
            scratch.extend_from_slice(&value.to_le_bytes()[..bytes]);
            offset += bytes;
        }
        destination.copy_from_slice(&scratch);
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        let implicit_stack_qword = context.origin == od_isa::memory::AccessOrigin::ImplicitStack
            && source.len() == 8
            && address & 7 == 0;
        let explicit_qword =
            context.origin == od_isa::memory::AccessOrigin::Explicit && source.len() == 8;
        let descriptor_table_byte =
            context.origin == od_isa::memory::AccessOrigin::DescriptorTable && source.len() == 1;
        if (!implicit_stack_qword && !explicit_qword && !descriptor_table_byte)
            || address & 0xfff > 0x1000 - source.len() as u64
        {
            return Err(MemoryError::Backend(
                "paged exception profile supports explicit/implicit-stack qword and descriptor-table byte writes only".into(),
            ));
        }
        let (width, value) = if implicit_stack_qword || explicit_qword {
            (
                AccessWidth::Qword,
                u64::from_le_bytes(source.try_into().unwrap()),
            )
        } else {
            (AccessWidth::Byte, u64::from(source[0]))
        };
        if descriptor_table_byte && !self.descriptor_mapping_matches_preflight(address, context) {
            return Err(MemoryError::Backend(
                "target descriptor A-bit write no longer maps to the validated physical byte"
                    .into(),
            ));
        }
        match self.access(address, context, RamAccessOperation::Write { width, value })? {
            TrackedAccessOutcome::Written => Ok(()),
            _ => Err(MemoryError::Backend(
                "tracked write returned a non-write outcome".into(),
            )),
        }
    }

    fn preflight_descriptor_accessed_write(
        &mut self,
        descriptor_access_address: u64,
        descriptor_context: AccessContext,
        stack_writes: &[(u64, usize)],
    ) -> Result<(), MemoryError> {
        let descriptor = self.bus.probe_tracked_ram_access(
            &self.paging,
            GuestLinearAddress::new(descriptor_access_address),
            AccessKind::Read,
            descriptor_context,
        );
        let Ok(ProbeOutcome::Translated(translation)) = descriptor.outcome else {
            return Err(MemoryError::Backend(
                "target descriptor read mapping could not be captured for preflight".into(),
            ));
        };

        let mut protected = Vec::with_capacity(5);
        if let Ok(byte) = GuestPhysicalRange::new(translation.physical, 1)
            && self.bus.tracked_physical_range_is_ram(byte)
        {
            protected.push(byte);
        }
        for entry in descriptor.trace.entries().iter().flatten() {
            if let Ok(range) = GuestPhysicalRange::new(entry.entry_address, 8)
                && self.bus.tracked_physical_range_is_ram(range)
            {
                protected.push(range);
            }
        }

        let mut reports = Vec::with_capacity(stack_writes.len());
        for &(linear, width) in stack_writes {
            let report = self.bus.probe_tracked_ram_access(
                &self.paging,
                GuestLinearAddress::new(linear),
                AccessKind::Write,
                AccessContext {
                    origin: od_isa::memory::AccessOrigin::ImplicitStack,
                    ..descriptor_context
                },
            );
            let succeeded = matches!(
                report.outcome,
                Ok(ProbeOutcome::Translated(translation))
                    if GuestPhysicalRange::new(translation.physical, width as u64)
                        .is_ok_and(|range| self.bus.tracked_physical_range_is_ram(range))
            );
            reports.push(report);
            // A later frame slot cannot affect delivery after the first failed
            // stack access. Probing it could mask that earlier architectural
            // fault with an alias refusal from an unreachable slot.
            if !succeeded {
                break;
            }
        }

        // A committed frame write can change a page-table entry used by a
        // later slot even when it does not directly overlap the GDT. Protect
        // every page-table dependency in the reachable prefix so the mappings
        // consumed after each frame write stay stable.
        let mut frame_protected = protected.clone();
        for report in &reports {
            for entry in report.trace.entries().iter().flatten() {
                if let Ok(range) = GuestPhysicalRange::new(entry.entry_address, 8)
                    && self.bus.tracked_physical_range_is_ram(range)
                    && !frame_protected.contains(&range)
                {
                    frame_protected.push(range);
                }
            }
        }

        for (report, &(_, width)) in reports.iter().zip(stack_writes) {
            let translated = match &report.outcome {
                Ok(ProbeOutcome::Translated(translation)) => Some(*translation),
                _ => None,
            };
            if let Some(translation) = translated
                && let Ok(operand) = GuestPhysicalRange::new(translation.physical, width as u64)
                && self.bus.tracked_physical_range_is_ram(operand)
                && overlaps_any(operand, &frame_protected)
            {
                return Err(descriptor_alias_error());
            }
            let trace_len = report.trace.len();
            let skip_last_accessed = match &report.outcome {
                Ok(ProbeOutcome::PageFault(fault)) => matches!(
                    fault.reason,
                    PageFaultReason::NotPresent { .. } | PageFaultReason::ReservedBits { .. }
                ),
                Err(ProbeError::UnsupportedPageSize { .. }) => true,
                _ => false,
            };
            for (index, entry) in report
                .trace
                .entries()
                .iter()
                .flatten()
                .take(trace_len)
                .enumerate()
            {
                let is_last = index + 1 == trace_len;
                let accessed_will_be_attempted =
                    entry.entry_value & (1 << 5) == 0 && !(is_last && skip_last_accessed);
                if accessed_will_be_attempted
                    && let Ok(range) = GuestPhysicalRange::new(entry.entry_address, 8)
                    && self.bus.tracked_physical_range_is_ram(range)
                    && overlaps_any(range, &protected)
                {
                    return Err(descriptor_alias_error());
                }
            }

            if let Some(translation) = translated
                && let Ok(operand) = GuestPhysicalRange::new(translation.physical, width as u64)
                && self.bus.tracked_physical_range_is_ram(operand)
                && let Some(leaf) = report.trace.entries().iter().flatten().last()
                && leaf.entry_value & (1 << 6) == 0
                && let Ok(dirty_entry) = GuestPhysicalRange::new(leaf.entry_address, 8)
                && self.bus.tracked_physical_range_is_ram(dirty_entry)
                && overlaps_any(dirty_entry, &protected)
            {
                return Err(descriptor_alias_error());
            }

            // A failed slot still commits any earlier Accessed writes from its
            // partial walk, but never its operand or a Dirty update.
        }

        self.descriptor_accessed_target = Some(translation.physical);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "atomic access is unsupported during exception delivery".into(),
        ))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "atomic access is unsupported during exception delivery".into(),
        ))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "atomic access is unsupported during exception delivery".into(),
        ))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "fence is unsupported during exception delivery".into(),
        ))
    }
}

fn overlaps_any(candidate: GuestPhysicalRange, protected: &[GuestPhysicalRange]) -> bool {
    protected.iter().any(|range| {
        u128::from(candidate.start().get()) < range.end_exclusive()
            && u128::from(range.start().get()) < candidate.end_exclusive()
    })
}

fn descriptor_alias_error() -> MemoryError {
    MemoryError::Backend(
        "target descriptor Accessed-bit write aliases a planned frame or page-table A/D write"
            .into(),
    )
}

fn access_width(bytes: usize) -> Result<AccessWidth, MemoryError> {
    match bytes {
        1 => Ok(AccessWidth::Byte),
        2 => Ok(AccessWidth::Word),
        4 => Ok(AccessWidth::Dword),
        8 => Ok(AccessWidth::Qword),
        _ => Err(MemoryError::Backend(
            "unsupported tracked descriptor read width".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_isa::memory::AccessOrigin;

    #[test]
    fn arbitrary_descriptor_reads_split_into_page_contained_accesses() {
        assert_eq!(access_width(1).unwrap(), AccessWidth::Byte);
        assert_eq!(access_width(8).unwrap(), AccessWidth::Qword);
    }

    #[test]
    fn implicit_stack_origin_keeps_supervisor_page_privilege() {
        let context = AccessContext {
            privilege: od_isa::memory::PrivilegeLevel::Ring0,
            origin: AccessOrigin::ImplicitStack,
            ..AccessContext::default()
        };
        assert_eq!(
            context.page_privilege(),
            od_isa::memory::PrivilegeLevel::Ring0
        );
    }
}
