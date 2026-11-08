//! S4-31b0: `build_dsdt` evaluated at compile time. The existing golden tests (`tests.rs`) pin
//! the runtime bytes against an independent reference; here the same manifest goes through the
//! const evaluator and must give identical bytes, so the const table equals the golden one.

use super::tests::reference_both;
use super::*;
use crate::acpi::manifest::{DeviceEntry, FLAG_PIO, FLAG_PRESENT, GSI_NONE};

const fn absent(device_type: u32) -> DeviceEntry {
    DeviceEntry {
        device_type,
        flags: 0,
        base: 0,
        length: 0,
        gsi: GSI_NONE,
        irq_flags: 0,
        param0: 0,
        param1: 0,
    }
}

/// The manifest of `dsdt_for_pci_root_and_com1_matches_the_independent_reference`: COM1 at
/// 0x3F8 (8 ports, GSI 4, edge, active-high) and a PCIe root with buses 0..=0xFF.
const MANIFEST: BoardManifest = {
    let com1 = DeviceEntry {
        flags: FLAG_PRESENT | FLAG_PIO,
        base: 0x3F8,
        length: 8,
        gsi: 4,
        param0: 1,
        ..absent(device_type::COM1)
    };
    let pci = DeviceEntry {
        flags: FLAG_PRESENT,
        base: 0xC000_0000,
        length: 0x1000_0000,
        param1: 0xFF << 8,
        ..absent(device_type::PCIE_ECAM)
    };
    let entries = [
        com1,
        absent(device_type::LOCAL_APIC),
        absent(device_type::IOAPIC),
        absent(device_type::DISPLAY),
        absent(device_type::NVRAM),
        absent(device_type::TPM_CRB),
        pci,
    ];
    match BoardManifest::from_entries(entries) {
        Ok(manifest) => manifest,
        Err(_) => panic!("from_entries failed in a const context"),
    }
};

/// The DSDT as static bytes plus its length, produced by the const `build_dsdt`.
const DSDT: ([u8; 256], usize) = {
    let mut out = [0u8; 256];
    let len = match build_dsdt(&MANIFEST, &mut out) {
        Ok(len) => len,
        Err(_) => panic!("build_dsdt failed in a const context"),
    };
    (out, len)
};

#[test]
fn const_dsdt_equals_the_runtime_dsdt() {
    let (bytes, len) = DSDT;
    let mut runtime = [0u8; 256];
    assert_eq!(build_dsdt(&MANIFEST, &mut runtime), Ok(len));
    assert_eq!(bytes, runtime);
}

#[test]
fn const_dsdt_equals_the_golden_reference_byte_for_byte() {
    // `reference_both` is the independent hand-checked table of the golden runtime test.
    let (bytes, len) = DSDT;
    assert_eq!(&bytes[..len], reference_both().as_slice());
    // Length 0xB0 and checksum 0x0E, checked separately.
    assert_eq!(len, 0xB0);
    assert_eq!(&bytes[4..8], &0xB0u32.to_le_bytes());
    assert_eq!(bytes[9], 0x0E);
    assert_eq!(validate_dsdt(&bytes[..len]), Ok(()));
}
