//! One-instruction diagnostic byte-PIO bridge through machine RAM and the bus.

use crate::{BusAccessError, MachineBus, MemoryRegionKind, PhysicalMemory};
use od_isa::execute::{DiagnosticCpl0PioU8V1, PioU8Result, step_pio_u8 as cpu_step_pio_u8};
use od_isa::memory::{AccessContext, GuestMemory, MemoryError};
use od_isa::{LongModeState, OperandSize};

/// Execute at most one bare `IN AL, DX` or `OUT DX, AL` through identity RAM
/// fetch and the machine's existing PIO bindings.
pub fn step_diagnostic_pio_u8(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    context: AccessContext,
    rflags_defined: u64,
) -> PioU8Result<BusAccessError> {
    let (memory, mut pio) = bus.split_for_pio_step();
    let mut memory = FlatFetchMemory { memory };
    cpu_step_pio_u8(
        DiagnosticCpl0PioU8V1,
        state,
        &mut memory,
        &mut pio,
        context,
        rflags_defined,
    )
}

struct FlatFetchMemory<'a> {
    memory: &'a PhysicalMemory,
}

impl FlatFetchMemory<'_> {
    fn unsupported(operation: &str) -> MemoryError {
        MemoryError::Backend(format!(
            "diagnostic PIO identity memory does not support {operation}"
        ))
    }
}

impl GuestMemory for FlatFetchMemory<'_> {
    fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        let address = crate::GuestPhysicalAddress::new(address);
        let kind = self
            .memory
            .locate_region_kind(address, 1)
            .map_err(|error| MemoryError::Backend(error.to_string()))?;
        if kind != MemoryRegionKind::Ram {
            return Err(MemoryError::Backend(format!(
                "instruction fetch from {kind:?} is unsupported in diagnostic PIO identity mode"
            )));
        }
        let mut byte = [0];
        self.memory
            .load_bytes(address, &mut byte)
            .map_err(|error| MemoryError::Backend(error.to_string()))?;
        Ok(byte[0])
    }

    fn read(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        Err(Self::unsupported("operand reads"))
    }

    fn write(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _source: &[u8],
    ) -> Result<(), MemoryError> {
        Err(Self::unsupported("operand writes"))
    }

    fn atomic_exchange_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported("atomic exchange"))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _addend: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported("atomic fetch-add"))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported("atomic compare-exchange"))
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Err(Self::unsupported("MFENCE"))
    }
}
