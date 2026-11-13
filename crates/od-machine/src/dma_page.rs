//! ISA DMA page register of channel 0 (`DMA_PAGE_0`, port `0x87`), S4-38L9w.
//!
//! Linux probes the legacy 8237A DMA controller in `i8237A_init_ops`
//! (`arch/x86/kernel/i8237.c`, a `device_initcall`) with exactly one port
//! access, `inb(DMA_PAGE_0)`: a read of `0xFF` means "the legacy DMA device is
//! removed" (`-ENODEV`), anything else lets the driver register its syscore
//! (suspend/resume) hooks. No other port of the DMA controller or of the page
//! register file (`0x00..0x0F`, `0x80..0x8F`, `0xC0..0xDF`) is touched on that
//! path; the resume hook, which writes them, runs only on a system resume.
//!
//! The model is the smallest honest answer to that read: one byte register
//! that is stored and read back, zero from construction. There is no DMA
//! controller behind it (no address, count or mask registers, no transfers):
//! the board does not model DMA, and the other page registers (`0x81..0x8F`)
//! stay unbound, i.e. traps, because the kernel does not touch them here.
//!
//! Width policy: byte accesses only, as in the other legacy port devices;
//! a word or dword access is refused with [`DeviceError::UnsupportedWidth`]
//! rather than made up. The board binds the device only under
//! `FirmwareBoardConfig::linux_profile`; without the flag `0x87` stays a trap.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};

/// `DMA_PAGE_0` of `arch/x86/include/asm/dma.h`: the page register of channel 0.
pub const DMA_PAGE_0_PORT: u16 = 0x87;
/// Ports of the `0x87` binding.
pub const DMA_PAGE_0_PORT_BYTES: u32 = 1;

/// One stored byte behind port `0x87`; the device sees offset 0.
#[derive(Debug, Default)]
pub struct DmaPage0 {
    value: u8,
}

impl DmaPage0 {
    pub const fn new() -> Self {
        Self { value: 0 }
    }

    /// Space, width and offset checks, in the order of the other PIO devices.
    fn check(access: DeviceAccess) -> Result<(), DeviceError> {
        if access.space != IoSpace::Pio {
            return Err(DeviceError::UnsupportedSpace);
        }
        if access.width != AccessWidth::Byte {
            return Err(DeviceError::UnsupportedWidth);
        }
        if access.offset >= u64::from(DMA_PAGE_0_PORT_BYTES) {
            return Err(DeviceError::UnknownOffset);
        }
        Ok(())
    }
}

impl BusDevice for DmaPage0 {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        Self::check(access)?;
        Ok(u64::from(self.value))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        Self::check(access)?;
        self.value = value as u8;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, PortAddress, PortRange, PortRegionBinding, RamLimits,
    };

    const BYTE: AccessWidth = AccessWidth::Byte;

    fn acc(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width,
        }
    }

    fn p(port: u16) -> PortAddress {
        PortAddress::new(port)
    }

    /// The device behind a real one-port binding at `0x87`.
    fn bus() -> MachineBus {
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let ports = vec![PortRegionBinding {
            range: PortRange::new(PortAddress::new(DMA_PAGE_0_PORT), DMA_PAGE_0_PORT_BYTES)
                .unwrap(),
            device_index: 0,
        }];
        MachineBus::new(
            map,
            RamLimits::new(1 << 20),
            vec![Box::new(DmaPage0::new())],
            Vec::new(),
            ports,
        )
        .unwrap()
    }

    #[test]
    fn constants_match_the_kernel_header() {
        assert_eq!(DMA_PAGE_0_PORT, 0x87);
        assert_eq!(DMA_PAGE_0_PORT_BYTES, 1);
    }

    #[test]
    fn reads_zero_by_default() {
        let mut dev = DmaPage0::new();
        assert_eq!(dev.read(acc(0, BYTE)), Ok(0));
        assert_eq!(DmaPage0::default().read(acc(0, BYTE)), Ok(0));
    }

    #[test]
    fn a_write_is_read_back_and_the_last_write_wins() {
        let mut dev = DmaPage0::new();
        for value in [0x01u64, 0x5A, 0xFF, 0x00, 0x80] {
            dev.write(acc(0, BYTE), value).unwrap();
            assert_eq!(dev.read(acc(0, BYTE)), Ok(value), "after {value:#x}");
            // A read has no side effect.
            assert_eq!(dev.read(acc(0, BYTE)), Ok(value));
        }
    }

    #[test]
    fn only_the_low_byte_of_a_write_is_stored() {
        let mut dev = DmaPage0::new();
        dev.write(acc(0, BYTE), 0x1_23).unwrap();
        assert_eq!(dev.read(acc(0, BYTE)), Ok(0x23));
    }

    #[test]
    fn refuses_wide_accesses_unknown_offsets_and_memory_space() {
        let mut dev = DmaPage0::new();
        for width in [AccessWidth::Word, AccessWidth::Dword] {
            assert_eq!(dev.read(acc(0, width)), Err(DeviceError::UnsupportedWidth));
            assert_eq!(
                dev.write(acc(0, width), 1),
                Err(DeviceError::UnsupportedWidth)
            );
        }
        assert_eq!(dev.read(acc(1, BYTE)), Err(DeviceError::UnknownOffset));
        assert_eq!(dev.write(acc(1, BYTE), 1), Err(DeviceError::UnknownOffset));
        let mmio = DeviceAccess {
            space: IoSpace::Mmio,
            offset: 0,
            width: BYTE,
        };
        assert_eq!(dev.read(mmio), Err(DeviceError::UnsupportedSpace));
        assert_eq!(dev.write(mmio, 1), Err(DeviceError::UnsupportedSpace));
        // The refused accesses stored nothing.
        assert_eq!(dev.read(acc(0, BYTE)), Ok(0));
    }

    #[test]
    fn the_bus_binding_stores_the_byte_and_leaves_neighbours_unmapped() {
        let mut bus = bus();
        assert_eq!(bus.read_port(p(0x87), BYTE), Ok(0));
        bus.write_port(p(0x87), BYTE, 0xA5).unwrap();
        assert_eq!(bus.read_port(p(0x87), BYTE), Ok(0xA5));
        // The write did not reach the other page registers or the POST port.
        for port in [0x80u16, 0x81, 0x83, 0x86, 0x88, 0x89, 0x8F, 0x0, 0xC0] {
            assert_eq!(
                bus.read_port(p(port), BYTE),
                Err(BusAccessError::UnmappedPort(p(port))),
                "read {port:#x}"
            );
        }
        // Wide accesses never reach a value.
        assert!(bus.read_port(p(0x87), AccessWidth::Word).is_err());
        assert_eq!(bus.read_port(p(0x87), BYTE), Ok(0xA5));
    }
}
