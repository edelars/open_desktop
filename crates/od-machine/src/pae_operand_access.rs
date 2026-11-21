//! The board's operand access of a vCPU that runs on PAE paging (S2-25j).
//!
//! The counterpart of [`LoadedFirmwareBoard::fetch_vcpu_view_pae`] for data: a load or a store of
//! 1, 2, 4 or 8 bytes at a 32-bit linear address of a vCPU whose legacy model is a protected-mode
//! one on PAE paging (`CR0.PG = 1`, `CR4.PAE = 1`, `EFER.LME = 0`) goes through the PAE walk of the
//! PDPTE registers of the model, commits the accessed bit of every entry the walk reaches and, for a
//! store, the dirty bit of the leaf, and is then served by the physical address: RAM and framebuffer
//! bytes directly, an MMIO operand by the device of the region as an access of that vCPU. The
//! long-mode path (`Long64ProbeConfig`, [`crate::MachineBus::access_tracked`]) is not selected or
//! changed by it.
//!
//! The decision on 36-bit physical addresses: the walk and the dispatch use the address the PAE
//! entries carry (up to 36 bits, `CPUID.80000008H:EAX` of the profile), not a 32-bit truncation. The
//! board's memory map decides what is there; an address that is not RAM, MMIO or framebuffer is an
//! `OperandPhysical` error (a model refusal, not a guest fault). The `od_isa` protected-mode
//! model, which addresses its memory with 32 bits, refuses such a page (`PhysicalBeyond4GiB`).
//!
//! Not here: an interpreter that decodes 32-bit instructions on this view (the only legacy
//! protected-mode executor is the closed profile of `ap_legacy_exec`, on RAM), a page-crossing
//! operand (`CrossesPage`, as in the 4-level path), locked operations, the descriptor-table
//! accesses.

// No production caller yet (like `pae_fetch_view`): tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::{
    GuestPhysicalRange, LoadedFirmwareBoard, PaeAccessRequest, PaeOperandOperation,
    TrackedAccessReport,
};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};

/// The outcome of [`LoadedFirmwareBoard::pae_operand_access`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PaeOperandAccess {
    /// The walk trace, the accessed/dirty updates and the operand result or error.
    pub(crate) report: TrackedAccessReport,
    /// Every committed write in order (the table entries of the accessed/dirty bits, then the
    /// operand bytes): what a caller turns into code invalidation.
    pub(crate) committed_writes: Vec<GuestPhysicalRange>,
}

impl LoadedFirmwareBoard {
    /// One operand access of vCPU `cpu` at the 32-bit linear address `linear` through its PAE paging.
    /// `instruction_rip` is the EIP recorded in a page-fault record. An error text when the vCPU does
    /// not exist, is not in a legacy protected-mode model or that model is not on PAE paging.
    pub(crate) fn pae_operand_access(
        &mut self,
        cpu: usize,
        instruction_rip: u64,
        linear: u32,
        privilege: PrivilegeLevel,
        operation: PaeOperandOperation,
    ) -> Result<PaeOperandAccess, String> {
        let paging = self.pae_paging_of(cpu)?;
        let request = PaeAccessRequest {
            instruction_rip,
            linear,
            context: AccessContext {
                mode: InstructionMode::Long64,
                privilege,
                origin: AccessOrigin::Explicit,
            },
            operation,
        };
        let mut committed_writes = Vec::new();
        let report = self
            .bus_mut()
            .access_tracked_pae(cpu, &paging, request, |range| committed_writes.push(range));
        Ok(PaeOperandAccess {
            report,
            committed_writes,
        })
    }
}
