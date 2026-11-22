//! The board's operand access of a vCPU that runs on 32-bit paging (S2-25 remainder).
//!
//! The counterpart of [`LoadedFirmwareBoard::fetch_vcpu_view_pg32`] for data: a load or a store of
//! 1, 2, 4 or 8 bytes at a 32-bit linear address of a vCPU whose legacy model is a protected-mode
//! one on 32-bit paging (`CR0.PG = 1`, `CR4.PAE = 0`) goes through the 32-bit walk of
//! `page_walk::pg32`, commits the accessed bit of every entry the walk reaches and, for a
//! store, the dirty bit of the leaf, and is then served by the physical address: RAM and framebuffer
//! bytes directly, an MMIO operand by the device of the region as an access of that vCPU. The
//! long-mode path (`Long64ProbeConfig`, [`crate::MachineBus::access_tracked`]) is not selected or
//! changed by it.
//!
//! The decision on 36-bit physical addresses: a 4 MiB page with the PSE-36 model switch carries up to
//! 36 address bits; the walk and the dispatch use that address, not a 32-bit truncation. The
//! board's memory map decides what is there; an address that is not RAM, MMIO or framebuffer is an
//! `OperandPhysical` error (a model refusal, not a guest fault). The `od_isa` protected-mode
//! model, which addresses its memory with 32 bits, refuses such a page (`PhysicalBeyond4GiB`).
//!
//! Not here: an interpreter that decodes 32-bit instructions on this view (the only legacy
//! protected-mode executor is the closed profile of `ap_legacy_exec`, on RAM), a page-crossing
//! operand (`CrossesPage`, as in the 4-level path), locked operations, the descriptor-table
//! accesses.

// No production caller yet (like `pg32_fetch_view`): tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::{
    GuestPhysicalRange, LoadedFirmwareBoard, PaeOperandOperation, Pg32AccessRequest,
    TrackedAccessReport,
};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};

/// The outcome of [`LoadedFirmwareBoard::pg32_operand_access`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Pg32OperandAccess {
    /// The walk trace, the accessed/dirty updates and the operand result or error.
    pub(crate) report: TrackedAccessReport,
    /// Every committed write in order (the table entries of the accessed/dirty bits, then the
    /// operand bytes): what a caller turns into code invalidation.
    pub(crate) committed_writes: Vec<GuestPhysicalRange>,
}

impl LoadedFirmwareBoard {
    /// One operand access of vCPU `cpu` at the 32-bit linear address `linear` through its 32-bit paging.
    /// `instruction_rip` is the EIP recorded in a page-fault record. An error text when the vCPU does
    /// not exist, is not in a legacy protected-mode model or that model is not on 32-bit paging.
    pub(crate) fn pg32_operand_access(
        &mut self,
        cpu: usize,
        instruction_rip: u64,
        linear: u32,
        privilege: PrivilegeLevel,
        operation: PaeOperandOperation,
    ) -> Result<Pg32OperandAccess, String> {
        let paging = self.pg32_paging_of(cpu)?;
        let request = Pg32AccessRequest {
            instruction_rip,
            linear,
            context: AccessContext {
                // Only fills the AccessContext shape: the 32-bit walk does not read `mode` (address
                // width, rights and the error code come from the paging model of the vCPU), so Long64
                // here does not select the long-mode path.
                mode: InstructionMode::Long64,
                privilege,
                origin: AccessOrigin::Explicit,
            },
            operation,
        };
        let mut committed_writes = Vec::new();
        let report = self
            .bus_mut()
            .access_tracked_pg32(cpu, &paging, request, |range| committed_writes.push(range));
        Ok(Pg32OperandAccess {
            report,
            committed_writes,
        })
    }
}
