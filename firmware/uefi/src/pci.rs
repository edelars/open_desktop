//! PCI configuration access through the ECAM window of bus 0 (handoff device code 7), shared by
//! the xHCI driver (XHCI_DEVICE.md sections 2 and 15) and the NVMe driver on PCI (S7-32).
//!
//! The configuration access is behind a trait so the scans and the BAR probe run in host tests;
//! the firmware instance reads and writes the identity-mapped ECAM window with 32-bit accesses.

use core::ptr;

use crate::{acpi::manifest::device_type, handoff::Handoff};

pub(crate) const COMMAND_REGISTER: u16 = 0x04;
pub(crate) const COMMAND_MEMORY_SPACE: u32 = 1 << 1;
pub(crate) const COMMAND_BUS_MASTER: u32 = 1 << 2;
pub(crate) const COMMAND_INTX_DISABLE: u32 = 1 << 10;
pub(crate) const CLASS_REGISTER: u16 = 0x08;
pub(crate) const BAR0_LOW: u16 = 0x10;
pub(crate) const BAR0_HIGH: u16 = 0x14;

/// ECAM window length for bus 0: 32 devices x 8 functions x 4 KiB.
pub(crate) const ECAM_BUS_BYTES: u64 = 1 << 20;

pub(crate) trait ConfigSpace {
    fn read(&self, device: u8, function: u8, register: u16) -> u32;
    fn write(&self, device: u8, function: u8, register: u16, value: u32);
}

/// ECAM offset of a configuration register on bus 0.
pub(crate) const fn ecam_offset(device: u8, function: u8, register: u16) -> u64 {
    ((device as u64) << 15) | ((function as u64) << 12) | register as u64
}

/// The identity-mapped ECAM window of the board (handoff device code 7).
pub(crate) struct Ecam {
    base: u64,
}

impl Ecam {
    /// # Safety
    /// `base` is the ECAM window of bus 0 (at least [`ECAM_BUS_BYTES`]), mapped as MMIO.
    pub(crate) const unsafe fn new(base: u64) -> Self {
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

/// The ECAM base of the board when the handoff carries a present window. `Err(length)`: the
/// entry is a PIO entry or shorter than bus 0.
pub(crate) fn ecam_window(handoff: &Handoff<'_>) -> Result<Option<u64>, u64> {
    let (devices, count) = handoff.device_entries();
    let Some(entry) = devices[..count]
        .iter()
        .find(|entry| entry.device_type == device_type::PCIE_ECAM && entry.is_present())
    else {
        return Ok(None);
    };
    if entry.is_pio() || entry.length < ECAM_BUS_BYTES {
        return Err(entry.length);
    }
    Ok(Some(entry.base))
}

/// The class code (base class, subclass, programming interface) of function 0 of `device`, or
/// `None` when the function is absent (the vendor or the whole identifier reads all ones).
pub(crate) fn function_class(config: &impl ConfigSpace, device: u8) -> Option<u32> {
    let id = config.read(device, 0, 0);
    (id != 0xffff_ffff && id & 0xffff != 0xffff)
        .then(|| config.read(device, 0, CLASS_REGISTER) >> 8)
}

/// Bus 0, function 0 of every device number: the first function of `class`.
pub(crate) fn find_class(config: &impl ConfigSpace, class: u32) -> Option<u8> {
    (0..32u8).find(|&device| function_class(config, device) == Some(class))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BarError {
    /// BAR0 is not a 64-bit memory BAR.
    NotMemory64,
    /// The probed size differs from the board contract.
    Size(u64),
    /// BAR0 is not programmed (address 0).
    Unassigned,
}

/// Reads BAR0 and checks its type and that its size is `expected` bytes. The firmware does not
/// reassign it (the board programs it at creation). The probe writes all ones to the BAR and
/// restores it, so the caller keeps Command.MSE clear while this runs.
pub(crate) fn probe_bar0(
    config: &impl ConfigSpace,
    device: u8,
    expected: u64,
) -> Result<u64, BarError> {
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
    if size != expected {
        return Err(BarError::Size(size));
    }
    let address = (u64::from(high) << 32) | u64::from(low & !0xf);
    if address == 0 {
        return Err(BarError::Unassigned);
    }
    Ok(address)
}

/// A bus-0 configuration space for host tests: function 0 of a few devices, BAR0 with a size mask.
#[cfg(test)]
pub(crate) mod fake {
    use super::{BAR0_HIGH, BAR0_LOW, COMMAND_REGISTER, ConfigSpace};
    use core::cell::RefCell;

    pub(crate) struct Fake {
        /// (device, words 0..8 = registers 0x00..0x20: id, command, class, header, BAR0 low/high).
        pub(crate) functions: RefCell<Vec<(u8, [u32; 8])>>,
        pub(crate) bar_mask: (u32, u32),
        /// The command register of the function at each BAR0 write (device, command): a probe
        /// must run with decoding and bus mastering off.
        pub(crate) bar_write_commands: RefCell<Vec<(u8, u32)>>,
    }

    impl Fake {
        /// The board: NVMe at device 4 (BAR0 0xFC004000, 16 KiB), xHCI at device 5 (0xFC008000).
        pub(crate) fn board() -> Self {
            let xhci = [0x0010_d1a6, 0, 0x0c03_3001, 0, 0xfc00_8004, 0, 0, 0];
            let nvme = [0x0020_d1a6, 0, 0x0108_0201, 0, 0xfc00_4004, 0, 0, 0];
            Self {
                functions: RefCell::new(vec![(4, nvme), (5, xhci)]),
                bar_mask: (0xffff_c000, 0xffff_ffff),
                bar_write_commands: RefCell::new(Vec::new()),
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
                if matches!(register, BAR0_LOW | BAR0_HIGH) {
                    let command = words[usize::from(COMMAND_REGISTER / 4)];
                    self.bar_write_commands.borrow_mut().push((device, command));
                }
                match register {
                    BAR0_LOW => words[slot] = (value & self.bar_mask.0) | (words[slot] & 0xf),
                    BAR0_HIGH => words[slot] = value & self.bar_mask.1,
                    _ => words[slot] = value,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    #[test]
    fn ecam_offsets_follow_the_bus_device_function_layout() {
        assert_eq!(ecam_offset(0, 0, 0), 0);
        assert_eq!(ecam_offset(5, 0, 0x10), 5 * 0x8000 + 0x10);
        assert_eq!(ecam_offset(31, 7, 0xfff), 0xf_ffff);
    }

    #[test]
    fn the_class_scan_skips_absent_functions() {
        let fake = Fake::board();
        assert_eq!(function_class(&fake, 4), Some(0x01_0802));
        assert_eq!(function_class(&fake, 6), None);
        assert_eq!(find_class(&fake, 0x01_0802), Some(4));
        assert_eq!(find_class(&fake, 0x02_0000), None);
    }

    #[test]
    fn bar_probe_checks_the_expected_size() {
        let fake = Fake::board();
        assert_eq!(probe_bar0(&fake, 4, 0x4000), Ok(0xfc00_4000));
        assert_eq!(probe_bar0(&fake, 4, 0x2000), Err(BarError::Size(0x4000)));
        assert_eq!(fake.read(4, 0, BAR0_LOW), 0xfc00_4004, "restored");
    }
}
