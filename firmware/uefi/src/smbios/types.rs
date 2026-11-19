//! SMBIOS formatted structures and their string areas (DSP0134 3.9, section 7).

use crate::{
    acpi::manifest::device_type,
    handoff::{Handoff, region_kind},
};

const VENDOR: &[u8] = b"Open Desktop";
const PRODUCT: &[u8] = b"Open Desktop Virtual Machine";
const BOARD_HANDLE: u16 = 0x0200;
const CHASSIS_HANDLE: u16 = 0x0300;
const ARRAY_HANDLE: u16 = 0x1000;
const MEMORY_HANDLE: u16 = 0x1100;

pub(super) fn build(
    handoff: &Handoff<'_>,
    processor_id: [u32; 2],
    out: &mut [u8],
) -> Option<usize> {
    let mut table = Writer { out, cursor: 0 };
    let (devices, count) = handoff.device_entries();
    let acpi = devices[..count]
        .iter()
        .any(|entry| entry.device_type == device_type::ACPI_PM && entry.is_present());

    let mut firmware = [0; 0x1a];
    firmware[4] = 1; // Vendor
    firmware[5] = 2; // Firmware Version
    firmware[8] = 3; // Firmware Release Date
    firmware[9] = 63; // 4 MiB ROM
    firmware[0x0b] = 1 << 4; // Firmware shadowing
    firmware[0x12] = u8::from(acpi);
    firmware[0x13] = (1 << 3) | (1 << 4); // UEFI, virtual machine
    firmware[0x14..0x18].copy_from_slice(&[0, 1, 0xff, 0xff]);
    firmware[0x18] = 4; // Extended ROM size, MiB
    table.record(
        0,
        0,
        firmware,
        &[VENDOR, env!("CARGO_PKG_VERSION").as_bytes(), b"11/19/2026"],
    )?;

    let mut system = [0; 0x1b];
    system[4] = 1;
    system[5] = 2;
    let serial = handoff.serial();
    system[7] = if serial.is_empty() { 0 } else { 3 };
    system[8..0x18].copy_from_slice(handoff.system_uuid());
    system[0x18] = 0x06; // Power switch (trial boot initiated by host).
    let strings = [VENDOR, PRODUCT, serial];
    let count = if serial.is_empty() { 2 } else { 3 };
    table.record(1, 0x0100, system, &strings[..count])?;

    let mut board = [0; 0x0f];
    board[4] = 1;
    board[5] = 2;
    board[9] = 1; // Hosting board
    board[0x0b..0x0d].copy_from_slice(&CHASSIS_HANDLE.to_le_bytes());
    board[0x0d] = 0x0a; // Motherboard
    table.record(2, BOARD_HANDLE, board, &[VENDOR, b"Open Desktop Board"])?;

    let mut chassis = [0; 0x16];
    chassis[4] = 1;
    chassis[5] = 0x01; // Other (virtual enclosure)
    chassis[9..0x0d].copy_from_slice(&[3, 3, 3, 3]); // Safe, no security state
    table.record(3, CHASSIS_HANDLE, chassis, &[VENDOR])?;

    let mut processor = [0; 0x33];
    processor[4..8].copy_from_slice(&[1, 3, 1, 2]); // Socket, CPU, Other, vendor
    processor[8..12].copy_from_slice(&processor_id[0].to_le_bytes());
    processor[12..16].copy_from_slice(&processor_id[1].to_le_bytes());
    processor[0x10] = 3; // Version
    let clock = handoff.virtual_cpu_mhz().to_le_bytes();
    processor[0x14..0x16].copy_from_slice(&clock); // Max Speed
    processor[0x16..0x18].copy_from_slice(&clock); // Current Speed
    processor[0x18..0x1a].copy_from_slice(&[0x41, 0x06]); // Enabled, no upgrade
    processor[0x1a..0x20].fill(0xff); // No cache structures
    let cores = handoff.cpu_count();
    processor[0x23..0x26].fill(cores as u8);
    processor[0x26] = 1 << 2; // 64-bit capable.
    processor[0x28] = processor[6]; // Family 2 agrees with the one-byte Family.
    for offset in [0x2a, 0x2c, 0x2e, 0x30] {
        processor[offset..offset + 2].copy_from_slice(&cores.to_le_bytes());
    }
    table.record(
        4,
        0x0400,
        processor,
        &[b"vCPU 0", VENDOR, b"Open Desktop x86-64"],
    )?;

    memory(handoff, &mut table)?;
    table.record(32, 0x2000, [0; 0x0b], &[])?;
    table.record(127, 0x7f00, [0; 4], &[])?;
    Some(table.cursor)
}

fn memory(handoff: &Handoff<'_>, table: &mut Writer<'_>) -> Option<()> {
    // Board-owned reservations in the low address space are carved out of
    // installed RAM. Include them when reconstructing the physical banks.
    let ranges = || {
        handoff.regions().filter(|region| {
            matches!(
                region.kind,
                region_kind::RAM
                    | region_kind::HANDOFF
                    | region_kind::BOOT_STACK
                    | region_kind::FIRMWARE_SHADOW
                    | region_kind::BOOT_PAGE_TABLES
                    | region_kind::ACPI_TABLES
                    | region_kind::ACPI_NVS
            )
        })
    };
    let bytes: u64 = ranges().map(|region| region.length).sum();
    let capacity_kib = (bytes >> 10) as u32; // Firmware board has a 36-bit GPA.
    let size_mib = (bytes >> 20) as u32; // RAM size is a multiple of 2 MiB.
    if size_mib == 0 {
        return None;
    }
    let mut array = [0; 0x17];
    array[4..7].copy_from_slice(&[3, 3, 3]); // System board, system memory, no ECC
    array[7..0x0b].copy_from_slice(&capacity_kib.to_le_bytes());
    array[0x0b..0x0d].copy_from_slice(&0xfffeu16.to_le_bytes());
    array[0x0d] = 1; // One on-board memory device
    table.record(16, ARRAY_HANDLE, array, &[])?;

    let mut device = [0; 0x64];
    device[4..6].copy_from_slice(&ARRAY_HANDLE.to_le_bytes());
    device[6..8].copy_from_slice(&0xfffeu16.to_le_bytes());
    device[8] = 64; // Width of the board's synthetic memory device
    device[0x0a] = 64;
    let legacy_size = size_mib.min(0x7fff) as u16;
    device[0x0c..0x0e].copy_from_slice(&legacy_size.to_le_bytes());
    if legacy_size == 0x7fff {
        device[0x1c..0x20].copy_from_slice(&size_mib.to_le_bytes());
    }
    device[0x0e] = 5; // Chip, on-board
    device[0x10] = 1; // Device locator
    device[0x12] = 7; // RAM
    device[0x13] = 1 << 1; // Other memory type detail
    device[0x28] = 1; // Other (virtual memory technology)
    device[0x29] = 1 << 3; // Volatile operating mode
    device[0x3c..0x44].copy_from_slice(&bytes.to_le_bytes());
    device[0x5f] = 0xff; // PMIC revision unknown
    device[0x63] = 0xff; // RCD revision unknown
    table.record(17, MEMORY_HANDLE, device, &[b"System Memory"])?;

    let mut ranges = ranges();
    let first = ranges.next()?;
    let mut base = first.base;
    let mut end = first.base + first.length;
    let mut bank = 0;
    for range in ranges {
        if range.base == end {
            end += range.length;
        } else {
            memory_mapping(table, base, end, bank)?;
            bank += 1;
            base = range.base;
            end = range.base + range.length;
        }
    }
    memory_mapping(table, base, end, bank)
}

fn memory_mapping(table: &mut Writer<'_>, base: u64, end: u64, bank: u16) -> Option<()> {
    // The legacy fields encode inclusive KiB addresses and cover the entire
    // supported board address space. Extended address fields stay zero.
    let start_kib = ((base >> 10) as u32).to_le_bytes();
    let end_kib = (((end - 1) >> 10) as u32).to_le_bytes();
    let array_mapping = 0x1300 + bank;
    let mut mapped_array = [0; 0x1f];
    mapped_array[4..8].copy_from_slice(&start_kib);
    mapped_array[8..12].copy_from_slice(&end_kib);
    mapped_array[12..14].copy_from_slice(&ARRAY_HANDLE.to_le_bytes());
    mapped_array[14] = 1;
    table.record(19, array_mapping, mapped_array, &[])?;

    let mut mapped_device = [0; 0x23];
    mapped_device[4..8].copy_from_slice(&start_kib);
    mapped_device[8..12].copy_from_slice(&end_kib);
    mapped_device[12..14].copy_from_slice(&MEMORY_HANDLE.to_le_bytes());
    mapped_device[14..16].copy_from_slice(&array_mapping.to_le_bytes());
    mapped_device[16] = 1; // First row
    table.record(20, 0x1400 + bank, mapped_device, &[])
}

struct Writer<'a> {
    out: &'a mut [u8],
    cursor: usize,
}

impl Writer<'_> {
    fn record<const N: usize>(
        &mut self,
        kind: u8,
        handle: u16,
        mut formatted: [u8; N],
        strings: &[&[u8]],
    ) -> Option<()> {
        formatted[0] = kind;
        formatted[1] = N as u8;
        formatted[2..4].copy_from_slice(&handle.to_le_bytes());
        self.append(&formatted)?;
        for string in strings {
            self.append(string)?;
            self.append(&[0])?;
        }
        if strings.is_empty() {
            self.append(&[0])?;
        }
        self.append(&[0])
    }

    fn append(&mut self, bytes: &[u8]) -> Option<()> {
        let end = self.cursor + bytes.len();
        self.out.get_mut(self.cursor..end)?.copy_from_slice(bytes);
        self.cursor = end;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::HANDOFF_MINOR_MAX;

    const BLOCK: &[u8] = include_bytes!("../../../../tests/fixtures/firmware/acpi_handoff.bin");

    fn word32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn fixed_board_identity_processor_and_ram_are_preserved() {
        let board = Handoff::parse_for_boot(BLOCK, HANDOFF_MINOR_MAX).unwrap();
        let mut bytes = [0; super::super::TABLE_CAPACITY];
        let size = build(&board, [0x600, 0], &mut bytes).unwrap();
        let mut table = &bytes[..size];
        for (kind, len) in [
            (0, 0x1a),
            (1, 0x1b),
            (2, 0x0f),
            (3, 0x16),
            (4, 0x33),
            (16, 0x17),
            (17, 0x64),
            (19, 0x1f),
            (20, 0x23),
            (32, 0x0b),
            (127, 4),
        ] {
            assert_eq!(&table[..2], &[kind, len as u8]);
            let (record, strings) = table.split_at(len);
            let end = strings.windows(2).position(|pair| pair == [0, 0]).unwrap();
            match kind {
                1 => {
                    assert_eq!(&record[8..0x18], &BLOCK[0x48..0x58]);
                    assert_eq!(record[7], 3);
                    assert!(strings[..end].ends_with(b"\0OD-S7-13"));
                }
                4 => {
                    assert_eq!(&record[8..16], b"\0\x06\0\0\0\0\0\0");
                    assert_eq!(&record[0x14..0x18], b"\xe8\x03\xe8\x03");
                    assert_eq!(&record[0x1a..0x20], &[0xff; 6]);
                    assert_eq!(&record[0x23..0x26], &[1, 1, 1]);
                }
                17 => {
                    assert_eq!(&record[4..6], &[0, 0x10]);
                    assert_eq!(record[0x29], 1 << 3);
                    assert_eq!(&record[0x3c..0x44], &(128u64 << 20).to_le_bytes());
                }
                20 => assert_eq!(&record[12..16], &[0, 0x11, 0, 0x13]),
                _ => {}
            }
            table = &strings[end + 2..];
        }
        assert!(table.is_empty());
        assert_eq!(build(&board, [0x600, 0], &mut bytes[..size - 1]), None);
    }

    #[test]
    fn expanded_ram_uses_extended_size_and_keeps_the_pci_hole() {
        const LOW_END: u64 = 0xf400_0000;
        const HIGH_BASE: u64 = 0x1_0000_0000;
        const HIGH_BYTES: u64 = 32 << 30;
        let mut handoff = BLOCK.to_vec();
        let start = word32(&handoff, 0x30) as usize;
        let count = word32(&handoff, 0x34);
        let device_offset = word32(&handoff, 0x3c) as usize;
        let low = (start..device_offset)
            .step_by(0x20)
            .filter(|&at| word32(&handoff, at) == region_kind::RAM)
            .last()
            .unwrap();
        let low_start = u64::from_le_bytes(handoff[low + 8..low + 16].try_into().unwrap());
        handoff[low + 16..low + 24].copy_from_slice(&(LOW_END - low_start).to_le_bytes());
        let mut high = [0; 0x20];
        high[..4].copy_from_slice(&region_kind::RAM.to_le_bytes());
        high[8..16].copy_from_slice(&HIGH_BASE.to_le_bytes());
        high[16..24].copy_from_slice(&HIGH_BYTES.to_le_bytes());
        high[24..28].fill(0xff);
        handoff.splice(device_offset..device_offset, high);
        let len = handoff.len() as u32;
        handoff[0x10..0x14].copy_from_slice(&len.to_le_bytes());
        handoff[0x34..0x38].copy_from_slice(&(count + 1).to_le_bytes());
        handoff[0x3c..0x40].copy_from_slice(&((device_offset + high.len()) as u32).to_le_bytes());
        let crc = crate::crc32::with_zeroed_word(&handoff, 0x14);
        handoff[0x14..0x18].copy_from_slice(&crc.to_le_bytes());

        let board = Handoff::parse(&handoff, HANDOFF_MINOR_MAX).unwrap();
        let mut bytes = [0; super::super::TABLE_CAPACITY];
        let len = build(&board, [0x600, 0], &mut bytes).unwrap();
        let mut table = &bytes[..len];
        let mut starts = Vec::new();
        while !table.is_empty() {
            let kind = table[0];
            let (record, strings) = table.split_at(table[1] as usize);
            if kind == 17 {
                assert_eq!(&record[0x0c..0x0e], &[0xff, 0x7f]);
                assert_eq!(word32(record, 0x1c), ((LOW_END + HIGH_BYTES) >> 20) as u32);
            }
            if kind == 19 {
                starts.push((word32(record, 4), word32(record, 8)));
            }
            let end = strings.windows(2).position(|pair| pair == [0, 0]).unwrap();
            table = &strings[end + 2..];
        }
        assert_eq!(
            starts,
            [
                (0, ((LOW_END - 1) >> 10) as u32),
                (
                    (HIGH_BASE >> 10) as u32,
                    ((HIGH_BASE + HIGH_BYTES - 1) >> 10) as u32
                ),
            ]
        );
    }
}
