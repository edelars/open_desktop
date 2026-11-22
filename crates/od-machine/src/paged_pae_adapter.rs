//! The PAE counterpart of [`crate::PagedMovMemoryAdapter`] (S2-25l): a `GuestMemory` for a vCPU that runs
//! on PAE paging (`CR0.PG = 1`, `CR4.PAE = 1`, `EFER.LME = 0`, the legacy protected-mode models of
//! `od-isa`), whose loads, stores and instruction bytes go through the three-level PAE walk of
//! [`crate::page_walk::pae`] and are then served by the physical address: RAM and framebuffer bytes
//! directly, an MMIO operand by the device of the region as an access of the vCPU.
//!
//! Why a sibling type and not a mode of `PagedMovMemoryAdapter`: that adapter carries a
//! [`crate::Long64ProbeConfig`] (whose constructor *requires* `EFER.LME = 1` and `EFER.LMA = 1`) and some
//! twenty Long64 walk sites (the fetch cache, the vector, locked, descriptor and cross-page paths, the
//! diagnostics), each of which would have to be guarded against a PAE snapshot; a site that was missed
//! would walk four levels silently over PAE tables. Here the paging snapshot is a
//! [`PaeProbeConfig`] by type, so no four-level walk is reachable from this adapter. The long-mode
//! adapter and its profiles are not changed.
//!
//! What it serves, and what it refuses with a typed [`MemoryError::Backend`] (never a guess):
//! - `fetch`: one byte, the walk of a fetch (accessed bits, `XD` with `EFER.NXE`), RAM or framebuffer only;
//! - `read` and `write` of 1, 2, 4 and 8 bytes. An operand inside one 4 KiB page is one walk and one
//!   access (MMIO included). An operand that crosses a page is served byte by byte with a walk of its own
//!   per byte (as the 4-level set loop does, S4-38L5e/L4h); a read assembles the bytes and hands them over
//!   after the last one, a write first walks both pages with write intent (no dirty bit) and only if both
//!   pass writes the bytes. A crossing operand never reaches a device: an MMIO byte is the typed error of
//!   an access without a dispatch;
//! - `read_integer_for_write` / `read_for_write_integer` inside one page (a crossing destination is
//!   `CrossesPage`), RAM only: an MMIO destination is refused before any device is reached;
//! - refused: every locked operation (`atomic_*`, `CMPXCHG16B`: there is no host atomic path over PAE
//!   tables here), `MFENCE` (as in the long-mode adapter), 16-byte vector operands, a descriptor-table
//!   access (the descriptor gate and transaction belong to the long-mode adapter), a linear address
//!   above 4 GiB and an operand that wraps past it. `fence` is forwarded to the memory like the
//!   long-mode adapter's.
//!
//! Page faults are recorded exactly as the long-mode adapter records them
//! ([`PagedPaeMemoryAdapter::page_fault`], the error code and CR2 of the PAE walk, SDM Vol. 3A
//! section 5.7), non-architectural refusals in [`PagedPaeMemoryAdapter::memory_error`]. As in the walk,
//! the accessed bit of an entry the walk reached stays set after a later fault of the same access.

use crate::bus::SetStepMmio;
use crate::page_walk::pae::{
    PaeAccessRequest, PaeOperandOperation, PaeProbeConfig, access_pae_ram,
    access_pae_with_mmio_windows,
};
use crate::{
    AccessWidth, GuestPhysicalRange, MachineBus, PhysicalMemory, TrackedAccessError,
    TrackedAccessOutcome, TrackedAccessReport,
};
use od_isa::OperandSize;
use od_isa::memory::{
    AccessContext, AccessOrigin, FenceKind, GuestMemory, MemoryError, PageFaultRecord,
};

#[cfg(test)]
mod tests;

/// What the adapter reads and writes through.
enum PaeAdapterBus<'a> {
    /// The physical memory alone: an MMIO operand is the typed error of an access without a device.
    Ram(&'a PhysicalMemory),
    /// `Ram` plus the MMIO dispatch of the set loop: an operand wholly inside one of its windows goes to
    /// the device (the dispatch is the view of one vCPU's set loop).
    RamMmio(&'a PhysicalMemory, &'a dyn SetStepMmio),
    /// The whole bus: an MMIO operand reaches the bound device of its region as an access of the vCPU
    /// (second member), like `MachineBus::access_tracked_pae`.
    Machine(&'a mut MachineBus, usize),
}

impl PaeAdapterBus<'_> {
    fn memory(&self) -> &PhysicalMemory {
        match self {
            Self::Ram(memory) | Self::RamMmio(memory, _) => memory,
            Self::Machine(bus, _) => bus.physical_memory(),
        }
    }
}

/// `GuestMemory` of one instruction attempt of a vCPU on PAE paging. See the module documentation.
pub struct PagedPaeMemoryAdapter<'a> {
    bus: PaeAdapterBus<'a>,
    paging: PaeProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
    access_error: Option<TrackedAccessError>,
}

impl<'a> PagedPaeMemoryAdapter<'a> {
    /// The adapter over the physical memory alone: no device is reachable, an MMIO operand is a typed
    /// refusal. `instruction_rip` is the EIP a page-fault record carries; `committed_write` receives every
    /// write of the access (the accessed and dirty bits of the entries, then the operand bytes), in order.
    pub fn over_memory(
        memory: &'a PhysicalMemory,
        paging: PaeProbeConfig,
        instruction_rip: u64,
        committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> Self {
        Self::new(
            PaeAdapterBus::Ram(memory),
            paging,
            instruction_rip,
            committed_write,
        )
    }

    /// The adapter over the whole bus: an MMIO operand wholly inside a region with a device binding
    /// reaches that device as an access of vCPU `vcpu`, with the dirty bit of a write set first.
    pub fn over_machine(
        bus: &'a mut MachineBus,
        vcpu: usize,
        paging: PaeProbeConfig,
        instruction_rip: u64,
        committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> Self {
        Self::new(
            PaeAdapterBus::Machine(bus, vcpu),
            paging,
            instruction_rip,
            committed_write,
        )
    }

    /// The adapter of the set loop's form: RAM plus the windows of `mmio` (the board's ECAM and
    /// Local APIC / IOAPIC dispatch of one vCPU's view).
    // No production caller yet (like `pae_operand_access`): the tests exercise it now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn over_memory_and_set_mmio(
        memory: &'a PhysicalMemory,
        mmio: &'a dyn SetStepMmio,
        paging: PaeProbeConfig,
        instruction_rip: u64,
        committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> Self {
        Self::new(
            PaeAdapterBus::RamMmio(memory, mmio),
            paging,
            instruction_rip,
            committed_write,
        )
    }

    fn new(
        bus: PaeAdapterBus<'a>,
        paging: PaeProbeConfig,
        instruction_rip: u64,
        committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> Self {
        Self {
            bus,
            paging,
            instruction_rip,
            committed_write,
            page_fault: None,
            memory_error: None,
            access_error: None,
        }
    }

    /// The EIP the next page-fault record carries (the instruction that is attempted).
    pub fn set_instruction_rip(&mut self, instruction_rip: u64) {
        self.instruction_rip = instruction_rip;
    }

    /// The paging snapshot of the adapter.
    pub fn paging(&self) -> &PaeProbeConfig {
        &self.paging
    }

    /// The record of the last architectural page fault (the saved EIP, CR2 and the error code).
    pub fn page_fault(&self) -> Option<&PageFaultRecord> {
        self.page_fault.as_ref()
    }

    /// The last non-architectural refusal of a walk or of the physical access (a model limit, a
    /// device error), as the typed error the walk reported.
    pub fn memory_error(&self) -> Option<&TrackedAccessError> {
        self.memory_error.as_ref()
    }

    /// The last tracked-access error of any kind, page faults included.
    pub fn access_error(&self) -> Option<&TrackedAccessError> {
        self.access_error.as_ref()
    }

    fn unsupported(operation: &'static str) -> MemoryError {
        MemoryError::Backend(format!("PAE paged adapter does not support {operation}"))
    }

    /// A locked operation: refused before any walk (nothing is walked, written or notified).
    fn locked_refusal(address: u64, operation: &'static str) -> MemoryError {
        MemoryError::Backend(format!(
            "{operation} at {address:#x} is refused on PAE paging: the paged adapter has no host \
             atomic path over PAE tables (S2-25l; the long-mode adapter is not selected)"
        ))
    }

    /// The 32-bit linear address of an operand of `length` bytes at `address`: a PAE linear address has
    /// 32 bits and an operand does not wrap past 4 GiB.
    fn linear32(address: u64, length: usize) -> Result<u32, MemoryError> {
        let last = address
            .checked_add(length as u64 - 1)
            .ok_or(MemoryError::AddressOverflow { address, length })?;
        if last > u64::from(u32::MAX) {
            return Err(MemoryError::Backend(format!(
                "PAE linear address {address:#x}+{length} is above 4 GiB (a 32-bit linear address)"
            )));
        }
        Ok(address as u32)
    }

    /// The context of the walk: the page privilege of the access with the origin normalized, as the
    /// long-mode adapter does. A descriptor-table access is refused (see the module documentation).
    fn walk_context(context: AccessContext) -> Result<AccessContext, MemoryError> {
        if context.origin == AccessOrigin::DescriptorTable {
            return Err(Self::unsupported("a descriptor-table access"));
        }
        Ok(AccessContext {
            origin: AccessOrigin::Explicit,
            privilege: context.page_privilege(),
            ..context
        })
    }

    fn width_of(length: usize) -> Option<AccessWidth> {
        match length {
            1 => Some(AccessWidth::Byte),
            2 => Some(AccessWidth::Word),
            4 => Some(AccessWidth::Dword),
            8 => Some(AccessWidth::Qword),
            _ => None,
        }
    }

    fn crosses_page(linear: u32, length: usize) -> bool {
        (linear & 0xfff) as usize + length > 4096
    }

    /// One tracked access. `mmio_allowed` false is the access of one byte of a crossing operand: it
    /// never reaches a device (the physical memory alone serves it).
    fn access(
        &mut self,
        operation: PaeOperandOperation,
        linear: u32,
        context: AccessContext,
        mmio_allowed: bool,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let request = PaeAccessRequest {
            instruction_rip: self.instruction_rip,
            linear,
            context,
            operation,
        };
        let paging = &self.paging;
        let committed = &mut *self.committed_write;
        let report: TrackedAccessReport = match &mut self.bus {
            PaeAdapterBus::Ram(memory) => access_pae_ram(memory, paging, request, committed),
            PaeAdapterBus::RamMmio(memory, mmio) => {
                if mmio_allowed {
                    access_pae_with_mmio_windows(
                        memory,
                        paging,
                        request,
                        committed,
                        mmio.windows(),
                        |address, width, operation| mmio.dispatch(address, width, operation),
                    )
                } else {
                    access_pae_ram(memory, paging, request, committed)
                }
            }
            PaeAdapterBus::Machine(bus, vcpu) => {
                if mmio_allowed {
                    bus.access_tracked_pae(*vcpu, paging, request, committed)
                } else {
                    access_pae_ram(bus.physical_memory(), paging, request, committed)
                }
            }
        };
        match report.outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                self.access_error = Some(error.clone());
                Err(self.map_access_error(error))
            }
        }
    }

    fn map_access_error(&mut self, error: TrackedAccessError) -> MemoryError {
        match error {
            TrackedAccessError::PageFault { record, .. } => {
                self.page_fault = Some(record.clone());
                MemoryError::Architectural(record.fault().clone())
            }
            error @ TrackedAccessError::MmioBackend { .. } => {
                let message = format!("tracked PAE MMIO access failed: {error:?}");
                self.memory_error = Some(error);
                MemoryError::Backend(message)
            }
            error => {
                let message = format!("tracked PAE RAM access failed: {error:?}");
                self.memory_error = Some(error);
                MemoryError::Backend(message)
            }
        }
    }

    fn read_value(
        &mut self,
        linear: u32,
        length: usize,
        width: AccessWidth,
        context: AccessContext,
    ) -> Result<u64, MemoryError> {
        if !Self::crosses_page(linear, length) {
            return match self.access(PaeOperandOperation::Read { width }, linear, context, true)? {
                TrackedAccessOutcome::Read(value) => Ok(value),
                _ => Err(Self::unsupported("non-read result")),
            };
        }
        // One walk per byte; the value is handed over only after the last byte was read.
        let mut bytes = [0u8; 8];
        for (index, byte) in bytes[..length].iter_mut().enumerate() {
            let outcome = self.access(
                PaeOperandOperation::Read {
                    width: AccessWidth::Byte,
                },
                linear + index as u32,
                context,
                false,
            )?;
            match outcome {
                TrackedAccessOutcome::Read(value) => *byte = value as u8,
                _ => return Err(Self::unsupported("non-read result")),
            }
        }
        Ok(u64::from_le_bytes(bytes))
    }
}

impl GuestMemory for PagedPaeMemoryAdapter<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        let linear = Self::linear32(address, 1)?;
        let context = Self::walk_context(context)?;
        match self.access(PaeOperandOperation::Fetch, linear, context, false)? {
            TrackedAccessOutcome::Fetched(value) => Ok(value),
            _ => Err(Self::unsupported("non-fetch result")),
        }
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        let length = destination.len();
        let width = Self::width_of(length).ok_or_else(|| Self::unsupported("read width"))?;
        let linear = Self::linear32(address, length)?;
        let context = Self::walk_context(context)?;
        let value = self.read_value(linear, length, width, context)?;
        destination.copy_from_slice(&value.to_le_bytes()[..length]);
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        let length = source.len();
        let width = Self::width_of(length).ok_or_else(|| Self::unsupported("write width"))?;
        let linear = Self::linear32(address, length)?;
        let context = Self::walk_context(context)?;
        if !Self::crosses_page(linear, length) {
            let mut bytes = [0u8; 8];
            bytes[..length].copy_from_slice(source);
            let value = u64::from_le_bytes(bytes);
            return match self.access(
                PaeOperandOperation::Write { width, value },
                linear,
                context,
                true,
            )? {
                TrackedAccessOutcome::Written => Ok(()),
                _ => Err(Self::unsupported("non-write result")),
            };
        }
        // Both pages are walked with write intent first (present, reserved, user and write-protection
        // checks, accessed bits, no dirty bit); only if both pass are the bytes written, each one
        // through a write access of its own.
        let second = (linear | 0xfff) + 1;
        for page_linear in [linear, second] {
            self.access(
                PaeOperandOperation::ReadForWrite {
                    width: AccessWidth::Byte,
                },
                page_linear,
                context,
                false,
            )?;
        }
        for (index, byte) in source.iter().enumerate() {
            let outcome = self.access(
                PaeOperandOperation::Write {
                    width: AccessWidth::Byte,
                    value: u64::from(*byte),
                },
                linear + index as u32,
                context,
                false,
            )?;
            if outcome != TrackedAccessOutcome::Written {
                return Err(Self::unsupported("non-write result"));
            }
        }
        Ok(())
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        let length = size.bytes();
        let width = Self::width_of(length).ok_or_else(|| Self::unsupported("read width"))?;
        let linear = Self::linear32(address, length)?;
        let context = Self::walk_context(context)?;
        match self.access(
            PaeOperandOperation::ReadForWrite { width },
            linear,
            context,
            false,
        )? {
            TrackedAccessOutcome::Read(value) => Ok(value),
            _ => Err(Self::unsupported("non-read result")),
        }
    }

    fn read_for_write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.read_integer_for_write(address, context, size)
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::locked_refusal(address, "a locked exchange"))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _addend: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::locked_refusal(address, "a locked add"))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::locked_refusal(address, "a locked compare-exchange"))
    }

    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::locked_refusal(address, "a locked read-modify-write"))
    }

    fn atomic_rmw_u128(
        &mut self,
        address: u64,
        _context: AccessContext,
        _update: &mut dyn FnMut(u128) -> u128,
    ) -> Result<u128, MemoryError> {
        Err(Self::locked_refusal(address, "CMPXCHG16B"))
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Err(Self::unsupported("MFENCE"))
    }

    fn fence(&mut self, kind: FenceKind) -> Result<(), MemoryError> {
        self.bus.memory().fence(kind);
        Ok(())
    }
}
