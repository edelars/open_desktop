//! One bounded Long64 CPL0 paged #PF delivery through a same-CPL IST=0 gate.

use crate::{
    AdUpdate, GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus,
    ProbeConfigError, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome,
    TrackedAccessRequest,
};
use od_isa::{
    LongModeState, OperandSize, PageFaultDeliveryResult, PagedCpl0Cr0WpMovV1, PagedProbeResult,
    deliver_page_fault,
    memory::{AccessContext, GuestMemory, MemoryError, PageFaultRecord},
    step_paged_cr0_wp_mov as cpu_step_paged_cr0_wp_mov,
};
use std::convert::Infallible;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedExceptionDeliveryError {
    Configuration(ProbeConfigError),
}

/// One delivery attempt, including the original fault and effects that remain
/// committed if a later table or stack access fails. A delivery abort is a
/// terminal undelivered result; this API does not classify #DF or shutdown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedExceptionDeliveryReport {
    pub original_fault: PageFaultRecord,
    pub delivery: PageFaultDeliveryResult,
    pub secondary_page_fault: Option<PageFaultRecord>,
    pub access_error: Option<TrackedAccessError>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
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
    };
    let delivery = deliver_page_fault(state, &mut memory, original_fault, context);
    Ok(PagedExceptionDeliveryReport {
        original_fault: original_fault.clone(),
        delivery,
        secondary_page_fault: memory.secondary_page_fault,
        access_error: memory.access_error,
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
    let handler_step = if matches!(delivery.delivery, PageFaultDeliveryResult::Delivered { .. }) {
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

struct PagedExceptionMemory<'a> {
    bus: &'a mut MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    secondary_page_fault: Option<PageFaultRecord>,
    access_error: Option<TrackedAccessError>,
    ad_updates: Vec<AdUpdate>,
    committed_writes: Vec<GuestPhysicalRange>,
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
        if source.len() != 8
            || (context.origin == od_isa::memory::AccessOrigin::ImplicitStack && address & 7 != 0)
            || address & 0xfff > 0xff8
        {
            return Err(MemoryError::Backend(
                "paged exception profile supports page-contained qword writes only".into(),
            ));
        }
        let value = u64::from_le_bytes(source.try_into().unwrap());
        match self.access(
            address,
            context,
            RamAccessOperation::Write {
                width: AccessWidth::Qword,
                value,
            },
        )? {
            TrackedAccessOutcome::Written => Ok(()),
            _ => Err(MemoryError::Backend(
                "tracked write returned a non-write outcome".into(),
            )),
        }
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
