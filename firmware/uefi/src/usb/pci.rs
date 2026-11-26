//! PCI discovery of the xHCI function through the ECAM window (XHCI_DEVICE.md section 2 and 15).
//!
//! The ECAM access, the class scan and the BAR probe are shared with the NVMe driver
//! ([`crate::pci`]); this module keeps the xHCI class and BAR size.

pub(super) use crate::pci::{
    BAR0_HIGH, BAR0_LOW, BarError, COMMAND_BUS_MASTER, COMMAND_INTX_DISABLE, COMMAND_MEMORY_SPACE,
    COMMAND_REGISTER, ConfigSpace, Ecam,
};

/// xHCI: base class 0x0C (serial bus), subclass 0x03 (USB), programming interface 0x30.
pub(super) const XHCI_CLASS: u32 = 0x0c_0330;
/// BAR0 size the board contract fixes (16 KiB).
pub(super) const XHCI_BAR_BYTES: u64 = 0x4000;

/// Bus 0, function 0 of every device number: the first xHCI function (class 0x0C0330).
/// An absent function reads all ones.
pub(super) fn find_xhci(config: &impl ConfigSpace) -> Option<u8> {
    crate::pci::find_class(config, XHCI_CLASS)
}

/// BAR0 of the xHCI function, checked against the board's 16 KiB (see [`crate::pci::probe_bar0`]).
pub(super) fn probe_bar0(config: &impl ConfigSpace, device: u8) -> Result<u64, BarError> {
    crate::pci::probe_bar0(config, device, XHCI_BAR_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci::fake::Fake;
    use core::cell::RefCell;

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
                bar_mask: (0, 0),
                bar_write_commands: RefCell::new(Vec::new()),
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
