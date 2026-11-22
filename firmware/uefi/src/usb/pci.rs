//! PCI discovery of the xHCI function through the ECAM window (XHCI_DEVICE.md section 2 and 15).
//!
//! The configuration access is behind a trait so the scan and the BAR probe run in host tests;
//! the firmware instance reads and writes the identity-mapped ECAM window with 32-bit accesses.

use core::ptr;

/// xHCI: base class 0x0C (serial bus), subclass 0x03 (USB), programming interface 0x30.
pub(super) const XHCI_CLASS: u32 = 0x0c_0330;
/// BAR0 size the board contract fixes (16 KiB).
pub(super) const XHCI_BAR_BYTES: u64 = 0x4000;

pub(super) const COMMAND_REGISTER: u16 = 0x04;
pub(super) const COMMAND_MEMORY_SPACE: u32 = 1 << 1;
pub(super) const COMMAND_BUS_MASTER: u32 = 1 << 2;
pub(super) const COMMAND_INTX_DISABLE: u32 = 1 << 10;
const CLASS_REGISTER: u16 = 0x08;
const BAR0_LOW: u16 = 0x10;
const BAR0_HIGH: u16 = 0x14;

/// ECAM window length for bus 0: 32 devices x 8 functions x 4 KiB.
pub(super) const ECAM_BUS_BYTES: u64 = 1 << 20;

pub(super) trait ConfigSpace {
    fn read(&self, device: u8, function: u8, register: u16) -> u32;
    fn write(&self, device: u8, function: u8, register: u16, value: u32);
}

/// ECAM offset of a configuration register on bus 0.
pub(super) const fn ecam_offset(device: u8, function: u8, register: u16) -> u64 {
    ((device as u64) << 15) | ((function as u64) << 12) | register as u64
}

/// The identity-mapped ECAM window of the board (handoff device code 7).
pub(super) struct Ecam {
    base: u64,
}

impl Ecam {
    /// # Safety
    /// `base` is the ECAM window of bus 0 (at least [`ECAM_BUS_BYTES`]), mapped as MMIO.
    pub(super) const unsafe fn new(base: u64) -> Self {
        Self { base }
    }

    fn address(&self, device: u8, function: u8, register: u16) -> usize {
        (self.base + ecam_offset(device, function, register & !3)) as usize
    }
}

impl ConfigSpace for Ecam {
    fn read(&self, device: u8, function: u8, register: u16) -> u32 {
        // SAFETY: `new` guarantees the window; accesses are aligned dwords.
        let value = unsafe {
            ptr::with_exposed_provenance::<u32>(self.address(device, function, register))
                .read_volatile()
        };
        // No read-modify-write folding: the firmware CPU profile has no memory-destination ALU.
        core::hint::black_box(value)
    }

    fn write(&self, device: u8, function: u8, register: u16, value: u32) {
        // SAFETY: as in `read`.
        unsafe {
            ptr::with_exposed_provenance_mut::<u32>(self.address(device, function, register))
                .write_volatile(value)
        };
    }
}

/// Bus 0, function 0 of every device number: the first xHCI function (class 0x0C0330).
/// An absent function reads all ones.
pub(super) fn find_xhci(config: &impl ConfigSpace) -> Option<u8> {
    (0..32u8).find(|&device| {
        let id = config.read(device, 0, 0);
        id != 0xffff_ffff
            && id & 0xffff != 0xffff
            && config.read(device, 0, CLASS_REGISTER) >> 8 == XHCI_CLASS
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BarError {
    /// BAR0 is not a 64-bit memory BAR.
    NotMemory64,
    /// The probed size differs from the board contract.
    Size(u64),
    /// BAR0 is not programmed (address 0).
    Unassigned,
}

/// Reads BAR0 and checks size and type. The firmware does not reassign it (the board
/// programs it at creation). The probe writes all ones to the BAR and restores it, so the
/// caller keeps Command.MSE clear while this runs.
pub(super) fn probe_bar0(config: &impl ConfigSpace, device: u8) -> Result<u64, BarError> {
    let low = config.read(device, 0, BAR0_LOW);
    let high = config.read(device, 0, BAR0_HIGH);
    // Bit 0 = 0: memory; bits 2:1 = 2: 64-bit.
    if low & 1 != 0 || (low >> 1) & 3 != 2 {
        return Err(BarError::NotMemory64);
    }
    config.write(device, 0, BAR0_LOW, 0xffff_ffff);
    config.write(device, 0, BAR0_HIGH, 0xffff_ffff);
    let mask_low = config.read(device, 0, BAR0_LOW);
    let mask_high = config.read(device, 0, BAR0_HIGH);
    config.write(device, 0, BAR0_LOW, low);
    config.write(device, 0, BAR0_HIGH, high);
    let mask = (u64::from(mask_high) << 32) | u64::from(mask_low & !0xf);
    let size = (!mask).wrapping_add(1);
    if size != XHCI_BAR_BYTES {
        return Err(BarError::Size(size));
    }
    let address = (u64::from(high) << 32) | u64::from(low & !0xf);
    if address == 0 {
        return Err(BarError::Unassigned);
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;

    /// A bus-0 configuration space with one xHCI-like function and optional others.
    struct Fake {
        functions: RefCell<Vec<(u8, [u32; 8])>>,
        bar_mask: (u32, u32),
    }

    impl Fake {
        fn board() -> Self {
            // Words 0..8 cover registers 0x00..0x20: id, command, class, header, BAR0 low/high.
            let xhci = [0x0010_d1a6, 0, 0x0c03_3001, 0, 0xfc00_8004, 0, 0, 0];
            let nvme = [0x0020_d1a6, 0, 0x0108_0201, 0, 0xfc00_4004, 0, 0, 0];
            Self {
                functions: RefCell::new(vec![(4, nvme), (5, xhci)]),
                bar_mask: (0xffff_c000, 0xffff_ffff),
            }
        }

        fn word(&self, device: u8, register: u16) -> Option<u32> {
            let functions = self.functions.borrow();
            let (_, words) = functions.iter().find(|(number, _)| *number == device)?;
            words.get(usize::from(register / 4)).copied()
        }
    }

    impl ConfigSpace for Fake {
        fn read(&self, device: u8, function: u8, register: u16) -> u32 {
            if function != 0 {
                return 0xffff_ffff;
            }
            self.word(device, register).unwrap_or(0xffff_ffff)
        }

        fn write(&self, device: u8, _function: u8, register: u16, value: u32) {
            let mut functions = self.functions.borrow_mut();
            if let Some((_, words)) = functions.iter_mut().find(|(number, _)| *number == device) {
                let slot = usize::from(register / 4);
                match register {
                    BAR0_LOW => words[slot] = (value & self.bar_mask.0) | (words[slot] & 0xf),
                    BAR0_HIGH => words[slot] = value & self.bar_mask.1,
                    _ => words[slot] = value,
                }
            }
        }
    }

    #[test]
    fn ecam_offsets_follow_the_bus_device_function_layout() {
        assert_eq!(ecam_offset(0, 0, 0), 0);
        assert_eq!(ecam_offset(5, 0, 0x10), 5 * 0x8000 + 0x10);
        assert_eq!(ecam_offset(31, 7, 0xfff), 0xf_ffff);
    }

    #[test]
    fn the_scan_matches_the_class_not_the_vendor() {
        let fake = Fake::board();
        assert_eq!(find_xhci(&fake), Some(5));
        fake.functions
            .borrow_mut()
            .retain(|(number, _)| *number != 5);
        assert_eq!(find_xhci(&fake), None);
        assert_eq!(
            find_xhci(&Fake {
                functions: RefCell::new(Vec::new()),
                bar_mask: (0, 0)
            }),
            None
        );
    }

    #[test]
    fn bar_probe_checks_the_size_and_restores_the_bar() {
        let fake = Fake::board();
        assert_eq!(probe_bar0(&fake, 5), Ok(0xfc00_8000));
        assert_eq!(fake.read(5, 0, BAR0_LOW), 0xfc00_8004);
        assert_eq!(fake.read(5, 0, BAR0_HIGH), 0);
    }

    #[test]
    fn bar_probe_rejects_wrong_size_type_and_unassigned_bars() {
        let mut fake = Fake::board();
        fake.bar_mask = (0xffff_8000, 0xffff_ffff);
        assert_eq!(probe_bar0(&fake, 5), Err(BarError::Size(0x8000)));
        // A 32-bit BAR (type bits 00).
        let fake = Fake::board();
        fake.functions.borrow_mut()[1].1[4] = 0xfc00_8000;
        assert_eq!(probe_bar0(&fake, 5), Err(BarError::NotMemory64));
        // An I/O BAR.
        fake.functions.borrow_mut()[1].1[4] = 0x0000_c001;
        assert_eq!(probe_bar0(&fake, 5), Err(BarError::NotMemory64));
        // 64-bit but at address 0.
        let fake = Fake::board();
        fake.functions.borrow_mut()[1].1[4] = 0x0000_0004;
        assert_eq!(probe_bar0(&fake, 5), Err(BarError::Unassigned));
    }
}
