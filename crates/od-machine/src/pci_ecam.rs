//! PCI ECAM window and type-0 configuration function model (S4-30a).
//!
//! One segment, one bus (0). The window is a plain MMIO region served by
//! [`PciEcam`]. No production function is registered; BAR decode is not mapped
//! into the physical memory map (stored bits only).

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use std::collections::BTreeMap;

pub const PCI_ECAM_BASE: u64 = 0xFC10_0000;
pub const PCI_ECAM_START_BUS: u8 = 0;
pub const PCI_ECAM_END_BUS: u8 = 0;
pub const PCI_ECAM_BYTES: u64 = 0x10_0000;

const COMMAND_RW_MASK: u32 = 0x0007;

/// A decoded ECAM address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EcamAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub register: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EcamAddressError {
    /// Offset is beyond the 256 buses addressable by ECAM.
    OutsideWindow,
    /// Bus number is outside `[PCI_ECAM_START_BUS, PCI_ECAM_END_BUS]`.
    InvalidBusNumber(u16),
}

/// Decodes an offset inside the ECAM window.
pub fn decode_ecam_offset(offset: u64) -> Result<EcamAddress, EcamAddressError> {
    let bus = offset >> 20;
    if bus > 0xFF {
        return Err(EcamAddressError::OutsideWindow);
    }
    if bus < u64::from(PCI_ECAM_START_BUS) || bus > u64::from(PCI_ECAM_END_BUS) {
        return Err(EcamAddressError::InvalidBusNumber(bus as u16));
    }
    Ok(EcamAddress {
        bus: bus as u8,
        device: ((offset >> 15) & 0x1F) as u8,
        function: ((offset >> 12) & 0x7) as u8,
        register: (offset & 0xFFF) as u16,
    })
}

/// A PCI configuration function. Accesses are already validated: width is
/// 1, 2 or 4 bytes and `register` is aligned to it.
pub trait PciFunction {
    fn config_read(&mut self, register: u16, width: AccessWidth) -> u32;
    fn config_write(&mut self, register: u16, width: AccessWidth, value: u32);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type0ConfigError {
    /// BAR size must be a power of two in `16..=2^31`.
    InvalidBarSize(u32),
    /// Class code is 24 bits (class, subclass, prog-if).
    InvalidClassCode(u32),
}

/// Type-0 configuration header with an optional 32-bit memory BAR0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Type0Config {
    vendor_id: u16,
    device_id: u16,
    revision_class: u32,
    command: u16,
    interrupt_line: u8,
    bar0_mask: u32,
    bar0: u32,
}

impl Type0Config {
    /// `class_code` is the 24-bit class/subclass/prog-if; `bar0_size` is the
    /// BAR0 window size in bytes, or `None` when BAR0 is not implemented.
    pub fn new(
        vendor_id: u16,
        device_id: u16,
        revision: u8,
        class_code: u32,
        bar0_size: Option<u32>,
    ) -> Result<Self, Type0ConfigError> {
        if class_code > 0xFF_FFFF {
            return Err(Type0ConfigError::InvalidClassCode(class_code));
        }
        let bar0_mask = match bar0_size {
            None => 0,
            Some(size) if size >= 16 && size.is_power_of_two() => !(size - 1) & 0xFFFF_FFF0,
            Some(size) => return Err(Type0ConfigError::InvalidBarSize(size)),
        };
        Ok(Self {
            vendor_id,
            device_id,
            revision_class: (class_code << 8) | u32::from(revision),
            command: 0,
            interrupt_line: 0,
            bar0_mask,
            bar0: 0,
        })
    }

    pub fn command(&self) -> u16 {
        self.command
    }

    pub fn bar0(&self) -> u32 {
        self.bar0
    }

    fn read_dword(&self, register: u16) -> u32 {
        match register {
            0x00 => u32::from(self.vendor_id) | (u32::from(self.device_id) << 16),
            0x04 => u32::from(self.command),
            0x08 => self.revision_class,
            0x0C => 0, // cache line, latency, header type 0x00, BIST
            0x10 => self.bar0,
            0x3C => u32::from(self.interrupt_line), // pin (0x3D) is RO 0
            _ => 0,
        }
    }

    fn write_dword(&mut self, register: u16, value: u32, byte_mask: u32) {
        let merge = |old: u32| (old & !byte_mask) | (value & byte_mask);
        match register {
            0x04 => {
                self.command = (merge(u32::from(self.command)) & COMMAND_RW_MASK) as u16;
            }
            0x10 => self.bar0 = merge(self.bar0) & self.bar0_mask,
            0x3C => self.interrupt_line = merge(u32::from(self.interrupt_line)) as u8,
            _ => {}
        }
    }
}

impl PciFunction for Type0Config {
    fn config_read(&mut self, register: u16, width: AccessWidth) -> u32 {
        let register = register & 0xFFF;
        if register >= 0x100 {
            return 0;
        }
        let shift = u32::from(register & 3) * 8;
        let value = self.read_dword(register & !3) >> shift;
        match width {
            AccessWidth::Byte => value & 0xFF,
            AccessWidth::Word => value & 0xFFFF,
            _ => value,
        }
    }

    fn config_write(&mut self, register: u16, width: AccessWidth, value: u32) {
        let register = register & 0xFFF;
        if register >= 0x100 {
            return;
        }
        let shift = u32::from(register & 3) * 8;
        let width_mask = match width {
            AccessWidth::Byte => 0xFF,
            AccessWidth::Word => 0xFFFF,
            _ => 0xFFFF_FFFF,
        };
        self.write_dword(register & !3, value << shift, width_mask << shift);
    }
}

/// ECAM window device. Holds functions by `(bus, device, function)`.
#[derive(Default)]
pub struct PciEcam {
    functions: BTreeMap<(u8, u8, u8), Box<dyn PciFunction>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PciEcamError {
    InvalidSlot,
    SlotOccupied,
    InvalidBusNumber,
}

impl PciEcam {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a single-function device. Only function 0 is accepted: the
    /// type-0 header has no multi-function bit, so enumerators never probe
    /// functions 1..=7 and they are not modelled.
    pub fn add_function(
        &mut self,
        bus: u8,
        device: u8,
        function: u8,
        config: Box<dyn PciFunction>,
    ) -> Result<(), PciEcamError> {
        if !(PCI_ECAM_START_BUS..=PCI_ECAM_END_BUS).contains(&bus) {
            return Err(PciEcamError::InvalidBusNumber);
        }
        if device > 31 || function != 0 {
            return Err(PciEcamError::InvalidSlot);
        }
        let key = (bus, device, function);
        if self.functions.contains_key(&key) {
            return Err(PciEcamError::SlotOccupied);
        }
        self.functions.insert(key, config);
        Ok(())
    }

    fn validate(access: DeviceAccess) -> Result<(EcamAddress, AccessWidth), DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let len = match access.width {
            AccessWidth::Qword => return Err(DeviceError::UnsupportedWidth),
            width => u64::from(width.bytes()),
        };
        if !access.offset.is_multiple_of(len) {
            return Err(DeviceError::Misaligned);
        }
        let address = decode_ecam_offset(access.offset).map_err(|_| DeviceError::UnknownOffset)?;
        Ok((address, access.width))
    }
}

impl BusDevice for PciEcam {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let (address, width) = Self::validate(access)?;
        let key = (address.bus, address.device, address.function);
        match self.functions.get_mut(&key) {
            Some(function) => Ok(u64::from(function.config_read(address.register, width))),
            None => Ok(match width {
                AccessWidth::Byte => 0xFF,
                AccessWidth::Word => 0xFFFF,
                _ => 0xFFFF_FFFF,
            }),
        }
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let (address, width) = Self::validate(access)?;
        let key = (address.bus, address.device, address.function);
        if let Some(function) = self.functions.get_mut(&key) {
            function.config_write(address.register, width, value as u32);
        }
        // Absent function: write is dropped (master abort), not an error.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    // diagnostic-only, не объявляется платой
    const TEST_VENDOR: u16 = 0xD1A6;
    const TEST_DEVICE: u16 = 0x0001;
    const TEST_CLASS_UNCLASSIFIED: u32 = 0xFF0000;
    const TEST_BAR0_SIZE: u32 = 4096;

    fn endpoint() -> Type0Config {
        Type0Config::new(
            TEST_VENDOR,
            TEST_DEVICE,
            1,
            TEST_CLASS_UNCLASSIFIED,
            Some(TEST_BAR0_SIZE),
        )
        .unwrap()
    }

    fn acc(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width,
        }
    }

    fn off(dev: u64, func: u64, reg: u64) -> u64 {
        (dev << 15) | (func << 12) | reg
    }

    fn ecam_with_endpoint() -> PciEcam {
        let mut ecam = PciEcam::new();
        ecam.add_function(0, 3, 0, Box::new(endpoint())).unwrap();
        ecam
    }

    use AccessWidth::{Byte, Dword, Qword, Word};

    #[test]
    fn window_does_not_overlap_other_devices() {
        use crate::display_device::{DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES};
        use crate::nvme::{NVME_MMIO_BASE, NVME_MMIO_BYTES};
        // Compile-time: the window starts after the display registers and the
        // NVMe window, and ends at or below 0xFEC0_0000 (the upper bound of the
        // range left unassigned in FIRMWARE_BOOT.md).
        const _: () = assert!(DISPLAY_REGISTERS_BASE + DISPLAY_REGISTERS_BYTES <= PCI_ECAM_BASE);
        const _: () = assert!(NVME_MMIO_BASE + NVME_MMIO_BYTES <= PCI_ECAM_BASE);
        const _: () = assert!(PCI_ECAM_BASE + PCI_ECAM_BYTES <= 0xFEC0_0000);
        assert_eq!(PCI_ECAM_BYTES, 1 << 20);
    }

    #[test]
    fn class_code_wider_than_24_bits_is_rejected() {
        assert_eq!(
            Type0Config::new(1, 1, 0, 0x0100_0000, None),
            Err(Type0ConfigError::InvalidClassCode(0x0100_0000))
        );
        assert!(Type0Config::new(1, 1, 0, 0xFF_FFFF, None).is_ok());
    }

    #[test]
    fn decode_edges() {
        let first = decode_ecam_offset(0).unwrap();
        assert_eq!(
            (first.bus, first.device, first.function, first.register),
            (0, 0, 0, 0)
        );
        let last = decode_ecam_offset(PCI_ECAM_BYTES - 1).unwrap();
        assert_eq!(
            (last.bus, last.device, last.function, last.register),
            (0, 31, 7, 4095)
        );
        let mid = decode_ecam_offset(off(5, 2, 0x10)).unwrap();
        assert_eq!((mid.device, mid.function, mid.register), (5, 2, 0x10));
        assert_eq!(
            decode_ecam_offset(PCI_ECAM_BYTES),
            Err(EcamAddressError::InvalidBusNumber(1))
        );
        assert_eq!(
            decode_ecam_offset(256 << 20),
            Err(EcamAddressError::OutsideWindow)
        );
        let mut ecam = PciEcam::new();
        assert_eq!(
            ecam.read(acc(PCI_ECAM_BYTES, Byte)),
            Err(DeviceError::UnknownOffset)
        );
        assert_eq!(
            ecam.add_function(1, 0, 0, Box::new(endpoint())),
            Err(PciEcamError::InvalidBusNumber)
        );
        assert_eq!(
            ecam.add_function(0, 32, 0, Box::new(endpoint())),
            Err(PciEcamError::InvalidSlot)
        );
        assert_eq!(
            ecam.add_function(0, 3, 1, Box::new(endpoint())),
            Err(PciEcamError::InvalidSlot)
        );
    }

    #[test]
    fn widths_and_alignment() {
        let mut ecam = ecam_with_endpoint();
        let base = off(3, 0, 0);
        assert_eq!(
            ecam.read(acc(base, Qword)),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(
            ecam.write(acc(base, Qword), 0),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(ecam.read(acc(base + 1, Word)), Err(DeviceError::Misaligned));
        assert_eq!(
            ecam.read(acc(base + 2, Dword)),
            Err(DeviceError::Misaligned)
        );
        assert_eq!(
            ecam.write(acc(base + 3, Word), 0),
            Err(DeviceError::Misaligned)
        );
        assert!(ecam.read(acc(base + 1, Byte)).is_ok());
        assert!(ecam.read(acc(base + 2, Word)).is_ok());
        assert!(ecam.read(acc(base + 4, Dword)).is_ok());
        let pio = DeviceAccess {
            space: IoSpace::Pio,
            offset: 0,
            width: Byte,
        };
        assert_eq!(ecam.read(pio), Err(DeviceError::UnsupportedSpace));
    }

    struct Counting(Rc<Cell<u32>>);
    impl PciFunction for Counting {
        fn config_read(&mut self, _: u16, _: AccessWidth) -> u32 {
            self.0.set(self.0.get() + 1);
            0
        }
        fn config_write(&mut self, _: u16, _: AccessWidth, _: u32) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn absent_function() {
        let calls = Rc::new(Cell::new(0));
        let mut ecam = PciEcam::new();
        ecam.add_function(0, 1, 0, Box::new(Counting(calls.clone())))
            .unwrap();
        ecam.add_function(0, 3, 0, Box::new(endpoint())).unwrap();
        for (dev, func) in [(0, 0), (1, 1), (2, 0), (31, 7), (3, 1)] {
            let o = off(dev, func, 0);
            assert_eq!(ecam.read(acc(o, Byte)), Ok(0xFF));
            assert_eq!(ecam.read(acc(o, Word)), Ok(0xFFFF));
            assert_eq!(ecam.read(acc(o, Dword)), Ok(0xFFFF_FFFF));
            ecam.write(acc(o + 4, Word), 0x7).unwrap();
            ecam.write(acc(o + 0x3C, Byte), 0x55).unwrap();
        }
        assert_eq!(calls.get(), 0);
        let present = off(3, 0, 4);
        assert_eq!(ecam.read(acc(present, Word)), Ok(0));
        assert_eq!(ecam.read(acc(off(3, 0, 0x3C), Byte)), Ok(0));
    }

    #[test]
    fn endpoint_identity_and_readonly() {
        let mut ecam = ecam_with_endpoint();
        let b = off(3, 0, 0);
        assert_eq!(ecam.read(acc(b, Word)), Ok(u64::from(TEST_VENDOR)));
        assert_eq!(ecam.read(acc(b + 2, Word)), Ok(u64::from(TEST_DEVICE)));
        assert_eq!(
            ecam.read(acc(b, Dword)),
            Ok(u64::from(TEST_VENDOR) | (u64::from(TEST_DEVICE) << 16))
        );
        assert_eq!(ecam.read(acc(b + 0x08, Dword)), Ok(0xFF00_0001));
        assert_eq!(ecam.read(acc(b + 0x0B, Byte)), Ok(0xFF));
        assert_eq!(ecam.read(acc(b + 0x0E, Byte)), Ok(0));
        assert_eq!(ecam.read(acc(b + 0x34, Byte)), Ok(0));
        // RO fields ignore writes.
        for (reg, w) in [
            (0x00, Dword),
            (0x02, Word),
            (0x06, Word),
            (0x08, Dword),
            (0x0E, Byte),
            (0x14, Dword),
            (0x27, Byte),
            (0x3D, Byte),
            (0x100, Dword),
            (0xFFC, Dword),
        ] {
            ecam.write(acc(b + reg, w), 0xFFFF_FFFF).unwrap();
        }
        assert_eq!(
            ecam.read(acc(b, Dword)),
            Ok(u64::from(TEST_VENDOR) | (u64::from(TEST_DEVICE) << 16))
        );
        assert_eq!(ecam.read(acc(b + 0x06, Word)), Ok(0));
        assert_eq!(ecam.read(acc(b + 0x08, Dword)), Ok(0xFF00_0001));
        assert_eq!(ecam.read(acc(b + 0x0E, Byte)), Ok(0));
        assert_eq!(ecam.read(acc(b + 0x14, Dword)), Ok(0));
        assert_eq!(ecam.read(acc(b + 0x3D, Byte)), Ok(0));
        assert_eq!(ecam.read(acc(b + 0x100, Dword)), Ok(0));
        assert_eq!(ecam.read(acc(b + 0xFFC, Dword)), Ok(0));
    }

    #[test]
    fn endpoint_command_and_interrupt_line() {
        let mut ecam = ecam_with_endpoint();
        let b = off(3, 0, 0);
        ecam.write(acc(b + 4, Word), 0xFFFF).unwrap();
        assert_eq!(ecam.read(acc(b + 4, Word)), Ok(0x7));
        assert_eq!(ecam.read(acc(b + 4, Dword)), Ok(0x7)); // status stays 0
        ecam.write(acc(b + 4, Dword), 0xFFFF_0002).unwrap();
        assert_eq!(ecam.read(acc(b + 4, Word)), Ok(0x2));
        ecam.write(acc(b + 4, Byte), 0x5).unwrap();
        assert_eq!(ecam.read(acc(b + 4, Word)), Ok(0x5));
        ecam.write(acc(b + 5, Byte), 0xFF).unwrap();
        assert_eq!(ecam.read(acc(b + 4, Word)), Ok(0x5));
        ecam.write(acc(b + 0x3C, Byte), 0xA5).unwrap();
        assert_eq!(ecam.read(acc(b + 0x3C, Byte)), Ok(0xA5));
        assert_eq!(ecam.read(acc(b + 0x3D, Byte)), Ok(0));
        ecam.write(acc(b + 0x3C, Dword), 0xFFFF_FF11).unwrap();
        assert_eq!(ecam.read(acc(b + 0x3C, Dword)), Ok(0x11));
    }

    #[test]
    fn endpoint_bar_sizing() {
        let mut ecam = ecam_with_endpoint();
        let bar = off(3, 0, 0x10);
        assert_eq!(ecam.read(acc(bar, Dword)), Ok(0));
        ecam.write(acc(bar, Dword), 0xFFFF_FFFF).unwrap();
        assert_eq!(ecam.read(acc(bar, Dword)), Ok(0xFFFF_F000));
        ecam.write(acc(bar, Dword), 0x1234_5678).unwrap();
        assert_eq!(ecam.read(acc(bar, Dword)), Ok(0x1234_5000));
        // RO bits 0..3 stay zero (memory, 32-bit, non-prefetchable).
        ecam.write(acc(bar, Dword), 0x0000_000F).unwrap();
        assert_eq!(ecam.read(acc(bar, Dword)).unwrap() & 0xF, 0);
        ecam.write(acc(bar, Dword), 0xFFFF_FFFF).unwrap();
        ecam.write(acc(bar, Dword), 0x8000_0000).unwrap();
        assert_eq!(ecam.read(acc(bar, Dword)), Ok(0x8000_0000));
        // Misaligned writes are rejected and change nothing.
        assert_eq!(
            ecam.write(acc(bar + 1, Dword), 0),
            Err(DeviceError::Misaligned)
        );
        assert_eq!(
            ecam.write(acc(bar + 2, Dword), 0),
            Err(DeviceError::Misaligned)
        );
        assert_eq!(ecam.read(acc(bar, Dword)), Ok(0x8000_0000));
        // Byte write merges and masks.
        ecam.write(acc(bar + 1, Byte), 0xFF).unwrap();
        assert_eq!(ecam.read(acc(bar, Dword)), Ok(0x8000_F000));
        // Other BARs are RO 0.
        for i in 1..6 {
            ecam.write(acc(off(3, 0, 0x10 + 4 * i), Dword), 0xFFFF_FFFF)
                .unwrap();
            assert_eq!(ecam.read(acc(off(3, 0, 0x10 + 4 * i), Dword)), Ok(0));
        }
    }

    #[test]
    fn invalid_bar_size_rejected() {
        for size in [0, 8, 24, 4097] {
            assert_eq!(
                Type0Config::new(1, 1, 0, 0, Some(size)),
                Err(Type0ConfigError::InvalidBarSize(size))
            );
        }
        let mut none = Type0Config::new(1, 1, 0, 0, None).unwrap();
        none.config_write(0x10, Dword, 0xFFFF_FFFF);
        assert_eq!(none.config_read(0x10, Dword), 0);
    }

    #[test]
    fn device_error_leaves_state_unchanged() {
        let mut ecam = ecam_with_endpoint();
        let b = off(3, 0, 0);
        ecam.write(acc(b + 0x3C, Byte), 0x42).unwrap();
        assert!(ecam.write(acc(b + 0x3C, Qword), 0xFF).is_err());
        assert!(ecam.write(acc(b + 0x3D, Dword), 0xFF).is_err());
        assert!(ecam.write(acc(PCI_ECAM_BYTES + 0x3C, Byte), 0xFF).is_err());
        assert_eq!(ecam.read(acc(b + 0x3C, Byte)), Ok(0x42));
    }

    fn bus_with_ecam() -> MachineBus {
        let range =
            |start, len| GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(0, 1 << 20), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(PCI_ECAM_BASE, PCI_ECAM_BYTES), MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        MachineBus::new(
            map,
            RamLimits::new(1 << 20),
            vec![Box::new(ecam_with_endpoint())],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(PCI_ECAM_BASE),
                device_index: 0,
            }],
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn through_machine_bus() {
        let mut bus = bus_with_ecam();
        let at = |o: u64| GuestPhysicalAddress::new(PCI_ECAM_BASE + o);
        assert_eq!(
            bus.read_physical(at(off(3, 0, 0)), Dword),
            Ok(u64::from(TEST_VENDOR) | (u64::from(TEST_DEVICE) << 16))
        );
        assert_eq!(bus.read_physical(at(off(4, 0, 0)), Dword), Ok(0xFFFF_FFFF));
        bus.write_physical(at(off(3, 0, 0x3C)), Byte, 0x7, |_| {})
            .unwrap();
        assert_eq!(bus.read_physical(at(off(3, 0, 0x3C)), Byte), Ok(0x7));
        assert!(matches!(
            bus.read_physical(at(off(3, 0, 0)), Qword),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedWidth,
                ..
            })
        ));
        assert!(matches!(
            bus.read_physical(at(off(3, 0, 1)), Word),
            Err(BusAccessError::Device {
                source: DeviceError::Misaligned,
                ..
            })
        ));
        // Outside the window never reaches the ECAM device.
        let outside = GuestPhysicalAddress::new(PCI_ECAM_BASE + PCI_ECAM_BYTES);
        assert!(bus.read_physical(outside, Byte).is_err());
        assert!(!matches!(
            bus.read_physical(outside, Byte),
            Err(BusAccessError::Device { .. })
        ));
        assert_eq!(bus.read_physical(at(off(3, 0, 0x3C)), Byte), Ok(0x7));
    }

    #[test]
    fn overlapping_registration_rejected() {
        let range =
            |start, len| GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(0, 1 << 20), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(PCI_ECAM_BASE, PCI_ECAM_BYTES), MemoryRegionKind::Mmio),
                MemoryRegionSpec::new(
                    range(PCI_ECAM_BASE + 0x8_0000, 0x1000),
                    MemoryRegionKind::Mmio,
                ),
            ],
        );
        assert!(map.is_err());
        // Binding the same region twice is rejected by the bus.
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(0, 1 << 20), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(PCI_ECAM_BASE, PCI_ECAM_BYTES), MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        let binding = || MmioBinding {
            region_start: GuestPhysicalAddress::new(PCI_ECAM_BASE),
            device_index: 0,
        };
        assert!(
            MachineBus::new(
                map,
                RamLimits::new(1 << 20),
                vec![Box::new(PciEcam::new())],
                vec![binding(), binding()],
                vec![],
            )
            .is_err()
        );
    }
}
