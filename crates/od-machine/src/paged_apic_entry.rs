//! S4-27i4a: caller-driven APIC interrupt entry on the paged Long64/CPL0 profile.
//!
//! One path serves both a wake from `HLT` (the CPU is `Halted`) and an entry at a completed
//! instruction boundary (the CPU is `Running`). It is the paged counterpart of
//! `DiagnosticHaltWait::poll_apic` for the flat profile and reuses the accepted S4-27h delivery
//! (`deliver_paged_external_interrupt`) unchanged: the caller requests the vector in the Local
//! APIC, this function selects it, delivers on a candidate state and publishes the candidate only
//! together with the IRR -> ISR acknowledge. Nothing is scheduled here.

use crate::{
    GuestPhysicalRange, LocalApicHandle, MachineBus, PagedExceptionDeliveryError,
    deliver_paged_external_interrupt,
};
use od_isa::memory::{AccessContext, MemoryError, PageFaultRecord};
use od_isa::{
    InterruptDeliveryPhase, InterruptDeliveryResult, InterruptOutsideProfile, LongModeState,
    StiShadow,
};

const IF: u64 = 1 << 9;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedApicEntryResult {
    /// IF=0, an STI shadow is active or no vector is eligible: nothing changed (a halted CPU
    /// keeps waiting).
    Idle,
    /// The interrupt frame is on the stack, the CPU runs the handler and the vector is in service.
    Entered { vector: u8, new_rsp: u64 },
    /// Delivery was refused before any CPU or memory commit; the vector stays in IRR and a halted
    /// CPU stays halted.
    OutsideProfile(InterruptOutsideProfile),
    /// A table or stack access faulted during delivery. Nothing was committed to the CPU and the
    /// vector stays in IRR; some stack stores may have reached memory, so the caller must stop
    /// (model: hardware raises #PF/#NP sequentially, nested delivery is S4-33).
    Aborted {
        phase: InterruptDeliveryPhase,
        cause: MemoryError,
    },
    /// The APIC selection changed between selection and acknowledge; the caller must stop.
    SelectionChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedApicEntryReport {
    pub result: PagedApicEntryResult,
    pub secondary_page_fault: Option<PageFaultRecord>,
    pub access_error: Option<crate::TrackedAccessError>,
    pub ad_updates: Vec<crate::AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

/// Deliver the APIC's eligible vector to the paged CPU at a caller-certified boundary (or from
/// `HLT`). The caller certifies one vCPU, private synchronous RAM and the exclusions of the S4-27h
/// entry. Preconditions on CR0/CR4/CR3/EFER are those of `deliver_paged_external_interrupt`.
#[allow(clippy::too_many_arguments)]
pub fn poll_paged_apic_entry(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    apic: &LocalApicHandle,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedApicEntryReport, PagedExceptionDeliveryError> {
    let idle = || PagedApicEntryReport {
        result: PagedApicEntryResult::Idle,
        secondary_page_fault: None,
        access_error: None,
        ad_updates: Vec::new(),
        committed_writes: Vec::new(),
    };
    if state.rflags & IF == 0 || state.sti_shadow == Some(StiShadow::Blocking) {
        return Ok(idle());
    }
    let Some(vector) = apic.eligible() else {
        return Ok(idle());
    };
    let mut candidate = state.clone();
    let report = deliver_paged_external_interrupt(
        &mut candidate,
        bus,
        cr3,
        efer,
        context,
        rflags_defined,
        vector,
        committed_write,
    )?;
    let result = match report.delivery {
        InterruptDeliveryResult::Delivered { new_rsp, .. } => {
            if apic.acknowledge_vector(vector, || *state = candidate) {
                PagedApicEntryResult::Entered { vector, new_rsp }
            } else {
                PagedApicEntryResult::SelectionChanged
            }
        }
        InterruptDeliveryResult::OutsideProfile(reason) => {
            PagedApicEntryResult::OutsideProfile(reason)
        }
        InterruptDeliveryResult::DeliveryAborted { phase, cause } => {
            PagedApicEntryResult::Aborted { phase, cause }
        }
    };
    Ok(PagedApicEntryReport {
        result,
        secondary_page_fault: report.secondary_page_fault,
        access_error: report.access_error,
        ad_updates: report.ad_updates,
        committed_writes: report.committed_writes,
    })
}
