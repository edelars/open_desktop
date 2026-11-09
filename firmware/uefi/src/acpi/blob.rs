//! Data-only ACPI table blob of the stage-4 board (S4-31d).
//!
//! The blob is produced entirely by `const fn` evaluation: the freestanding binary
//! `od-acpi-blob` places the result in the `.acpi_blob` section, `tools/build/
//! build_stage4_acpi_blob.py` only copies that section out and reads the header. Neither the
//! packaging nor the machine serializes a table of its own; the machine side only validates
//! (`od-machine`, `acpi_blob.rs`).
//!
//! Layout (little-endian, `HEADER_LEN` = 0x60 bytes of header, blob version 2):
//!
//! | offset | size | field |
//! |--------|------|-------|
//! | 0x00 | 8 | magic `ODACPIB1` |
//! | 0x08 | 4 | `version` (2) |
//! | 0x0C | 4 | `header_len` (0x60) |
//! | 0x10 | 32 | `manifest_hash`: SHA-256 of [`manifest_hash`] over the device table |
//! | 0x30 | 8 | `load_base`: GPA of payload offset 0 (0xE_0000) |
//! | 0x38 | 4 | `payload_offset`: offset of the payload in the blob |
//! | 0x3C | 4 | `payload_len`: bytes loaded at `load_base` (16-aligned) |
//! | 0x40 | 8 | `rsdp_address`: GPA of the RSDP (= `load_base`, payload offset 0) |
//! | 0x48 | 4 | `table_count` |
//! | 0x4C | 4 | `total_len`: length of the whole blob |
//! | 0x50 | 4 | `facs_offset`: offset of the FACS in the blob (= `payload_offset + payload_len`) |
//! | 0x54 | 4 | `facs_len`: 64 |
//! | 0x58 | 8 | `facs_address`: GPA of the FACS (0xE_F000, the NVS part) |
//!
//! After the header come `table_count` directory entries of 16 bytes (`signature[4]`,
//! `offset: u32` inside the payload, `length: u32`, reserved `u32` = 0). The payload starts at
//! `payload_offset` (16-aligned) and is loaded as is at `load_base` (the ACPI reclaim part,
//! `[0xE_0000, 0xE_F000)`):
//!
//! | payload offset | content |
//! |----------------|---------|
//! | 0x00 | RSDP revision 2 (36 bytes, the rest of the 0x40-byte slot is zero) |
//! | 0x40 | DSDT (with `\_S5`), then MADT (`APIC`), FADT (`FACP`), MCFG (when the ECAM window is present), XSDT; each table 16-aligned, gaps zero |
//!
//! The FACS cannot lie inside the payload: it must be at `0xE_F000`, in the NVS part of the
//! region (E820 type NVS, FACS is writable by the OS), while the payload is E820 reclaim and has
//! to end at or below `0xE_F000`; a payload that reached `0xE_F000` would carry a 60 KiB zero
//! gap. So the FACS is a second, separate segment of the blob: the 64 FACS bytes follow the
//! payload in the file (`facs_offset`) and are loaded at `facs_address`. The FACS has no
//! checksum and is not in the directory (it is not an SDT). The FADT points to it with
//! `FIRMWARE_CTRL`/`X_FIRMWARE_CTRL`, to the DSDT with `DSDT`/`X_DSDT`; the XSDT lists FADT,
//! MADT, MCFG (not DSDT and not FACS); the RSDP gives the XSDT address.

use super::bytes::{copy_at, fill_range, head_mut, sub, tail_mut, tri};
use super::dsdt::{DsdtError, build_dsdt_with_s5};
use super::fadt::{FACS_LEN, FacsError, FadtAddresses, FadtError, build_facs, build_fadt};
use super::madt::{MadtError, build_madt};
use super::manifest::{
    AcpiPmEntry, BoardManifest, DEVICE_ENTRY_SIZE, DeviceEntry, FLAG_PIO, FLAG_PRESENT, GSI_NONE,
    ManifestError, device_type,
};
use super::mcfg::{McfgError, build_mcfg};
use super::rsdp::{RsdpError, build_rsdp, build_xsdt};
use super::sha256::sha256;

pub const BLOB_MAGIC: [u8; 8] = *b"ODACPIB1";
pub const BLOB_VERSION: u32 = 2;
pub const HEADER_LEN: usize = 0x60;
pub const DIR_ENTRY_LEN: usize = 16;
/// Bytes at payload offset 0 reserved for the RSDP (36 bytes rounded up to a 16-aligned slot).
pub const RSDP_SLOT_LEN: usize = 0x40;
pub const TABLE_ALIGN: usize = 16;
pub const MAX_TABLES: usize = 8;
/// GPA of the FACS: the start of the NVS part of the region (decision 6 of S4-31).
pub const FACS_ADDRESS: u64 = ACPI_RECLAIM_END;

/// GPA of payload offset 0: the start of the ACPI table region (decision 6 of S4-31).
pub const ACPI_REGION_BASE: u64 = 0x000E_0000;
/// End of the whole ACPI region, FACS included.
pub const ACPI_REGION_END: u64 = 0x000F_0000;
/// End of the part for the tables (E820 ACPI reclaim); FACS (NVS) starts here.
pub const ACPI_RECLAIM_END: u64 = 0x000E_F000;

/// Domain tag of the manifest hash; the trailing digit is the hash scheme version.
pub const MANIFEST_HASH_TAG: [u8; 16] = *b"ODACPI-MANIFEST1";
/// Highest device type code the hash accepts (1..=15 are reserved by FIRMWARE_BOOT.md 6.3).
const MAX_DEVICE_TYPE: u32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobError {
    Manifest(ManifestError),
    Dsdt(DsdtError),
    Mcfg(McfgError),
    Fadt(FadtError),
    Facs(FacsError),
    Madt(MadtError),
    /// RSDP or XSDT.
    Rsdp(RsdpError),
    /// The device table has no ACPI fixed hardware entry (type 9).
    MissingAcpiPm,
    /// `out` cannot hold the header, the directory, the tables and the FACS.
    BufferTooSmall,
}

const fn align_up(value: usize, align: usize) -> usize {
    value.div_ceil(align) * align
}

/// SHA-256 over `MANIFEST_HASH_TAG` and the 48-byte encoding of every entry, ordered by type
/// code (so the hash does not depend on the order of the table). Types must be in 1..=15 and
/// unique; the hash covers the whole resolved device table, so any change of the board
/// configuration (a present device, a base, a flag) makes a blob built earlier foreign.
pub const fn manifest_hash(devices: &[DeviceEntry]) -> Result<[u8; 32], ManifestError> {
    let mut seen = [false; MAX_DEVICE_TYPE as usize + 1];
    let mut i = 0;
    while i < devices.len() {
        let t = devices[i].device_type;
        if t == 0 || t > MAX_DEVICE_TYPE {
            return Err(ManifestError::UnknownDeviceType(t));
        }
        if seen[t as usize] {
            return Err(ManifestError::DeviceTypeNotUnique(t));
        }
        seen[t as usize] = true;
        i += 1;
    }
    let mut buf = [0u8; 16 + MAX_DEVICE_TYPE as usize * DEVICE_ENTRY_SIZE];
    copy_at(&mut buf, 0, &MANIFEST_HASH_TAG);
    let mut len = MANIFEST_HASH_TAG.len();
    let mut t = 1;
    while t <= MAX_DEVICE_TYPE {
        let mut j = 0;
        while j < devices.len() {
            if devices[j].device_type == t {
                copy_at(&mut buf, len, &devices[j].to_bytes());
                len += DEVICE_ENTRY_SIZE;
            }
            j += 1;
        }
        t += 1;
    }
    Ok(sha256(sub(&buf, 0, len)))
}

/// The board manifest of the tables (types 1..=7) and the validated PM entry (type 9).
const fn split_devices(devices: &[DeviceEntry]) -> Result<(BoardManifest, AcpiPmEntry), BlobError> {
    let filler = DeviceEntry {
        device_type: 0,
        flags: 0,
        base: 0,
        length: 0,
        gsi: 0,
        irq_flags: 0,
        param0: 0,
        param1: 0,
    };
    let mut base_entries = [filler; 7];
    let mut count = 0;
    let mut pm: Option<AcpiPmEntry> = None;
    let mut i = 0;
    while i < devices.len() {
        let entry = devices[i];
        i += 1;
        if entry.device_type == device_type::ACPI_PM {
            pm = Some(tri!(AcpiPmEntry::from_entry(&entry), BlobError::Manifest));
        } else if entry.device_type >= 1 && entry.device_type <= 7 && count < 7 {
            base_entries[count] = entry;
            count += 1;
        }
    }
    let manifest = tri!(
        BoardManifest::from_entries(base_entries),
        BlobError::Manifest
    );
    match pm {
        Some(pm) => Ok((manifest, pm)),
        None => Err(BlobError::MissingAcpiPm),
    }
}

/// Zeroes the alignment gap after a table of `len` bytes at payload offset `cursor` and returns
/// the payload offset of the next 16-aligned slot.
const fn close_table(
    out: &mut [u8],
    payload_offset: usize,
    cursor: usize,
    len: usize,
) -> Result<usize, BlobError> {
    let next = align_up(cursor + len, TABLE_ALIGN);
    tri!(room(out, payload_offset + next));
    fill_range(out, payload_offset + cursor + len, payload_offset + next, 0);
    Ok(next)
}

/// `Err(BufferTooSmall)` unless `out` has `needed` bytes.
const fn room(out: &[u8], needed: usize) -> Result<(), BlobError> {
    if out.len() < needed {
        Err(BlobError::BufferTooSmall)
    } else {
        Ok(())
    }
}

/// Builds the blob for a device table into `out` and returns its length. Every byte the blob
/// owns is written (`out` may hold anything on entry), so the result depends on `devices` only.
///
/// A `const fn`: evaluated at compile time it gives the static section contents; called at run
/// time it gives the same bytes. The payload order is RSDP slot, DSDT (with `\_S5`), MADT, FADT,
/// MCFG (only with a `PRESENT` PCIe ECAM entry), XSDT; the FACS follows the payload as a
/// separate segment loaded at 0xE_F000 (see the module comment).
pub const fn build_blob(devices: &[DeviceEntry], out: &mut [u8]) -> Result<usize, BlobError> {
    let hash = tri!(manifest_hash(devices), BlobError::Manifest);
    let (manifest, pm) = tri!(split_devices(devices));
    let with_mcfg = match manifest.device(device_type::PCIE_ECAM) {
        Some(ecam) => ecam.is_present(),
        None => false,
    };
    let table_count = if with_mcfg { 5 } else { 4 };
    let payload_offset = align_up(HEADER_LEN + table_count * DIR_ENTRY_LEN, TABLE_ALIGN);
    tri!(room(out, payload_offset + RSDP_SLOT_LEN));
    // Header, directory and the RSDP slot start as zero; the gaps between tables are filled
    // with zero as they appear.
    fill_range(out, 0, payload_offset + RSDP_SLOT_LEN, 0);

    let mut signatures = [[0u8; 4]; MAX_TABLES];
    let mut offsets = [0u32; MAX_TABLES];
    let mut lengths = [0u32; MAX_TABLES];
    let mut cursor = RSDP_SLOT_LEN;
    let mut index = 0;

    tri!(room(out, payload_offset + cursor));
    let dsdt_at = cursor;
    let dsdt_len = tri!(
        build_dsdt_with_s5(
            &manifest,
            pm.slp_typ_s5,
            tail_mut(out, payload_offset + cursor)
        ),
        BlobError::Dsdt
    );
    signatures[index] = *b"DSDT";
    offsets[index] = cursor as u32;
    lengths[index] = dsdt_len as u32;
    index += 1;
    cursor = tri!(close_table(out, payload_offset, cursor, dsdt_len));

    tri!(room(out, payload_offset + cursor));
    let madt_at = cursor;
    let madt_len = tri!(
        build_madt(&manifest, &pm, tail_mut(out, payload_offset + cursor)),
        BlobError::Madt
    );
    signatures[index] = *b"APIC";
    offsets[index] = cursor as u32;
    lengths[index] = madt_len as u32;
    index += 1;
    cursor = tri!(close_table(out, payload_offset, cursor, madt_len));

    tri!(room(out, payload_offset + cursor));
    let fadt_at = cursor;
    let addresses = FadtAddresses {
        facs: FACS_ADDRESS,
        dsdt: ACPI_REGION_BASE + dsdt_at as u64,
    };
    let fadt_len = tri!(
        build_fadt(&pm, &addresses, tail_mut(out, payload_offset + cursor)),
        BlobError::Fadt
    );
    signatures[index] = *b"FACP";
    offsets[index] = cursor as u32;
    lengths[index] = fadt_len as u32;
    index += 1;
    cursor = tri!(close_table(out, payload_offset, cursor, fadt_len));

    let mut mcfg_at = 0;
    if with_mcfg {
        tri!(room(out, payload_offset + cursor));
        mcfg_at = cursor;
        let mcfg_len = tri!(
            build_mcfg(&manifest, tail_mut(out, payload_offset + cursor)),
            BlobError::Mcfg
        );
        signatures[index] = *b"MCFG";
        offsets[index] = cursor as u32;
        lengths[index] = mcfg_len as u32;
        index += 1;
        cursor = tri!(close_table(out, payload_offset, cursor, mcfg_len));
    }

    // XSDT: FADT, MADT and MCFG, in this order, by their addresses in the payload.
    let mut entries = [0u64; 3];
    entries[0] = ACPI_REGION_BASE + fadt_at as u64;
    entries[1] = ACPI_REGION_BASE + madt_at as u64;
    let entry_count = if with_mcfg {
        entries[2] = ACPI_REGION_BASE + mcfg_at as u64;
        3
    } else {
        2
    };
    tri!(room(out, payload_offset + cursor));
    let xsdt_at = cursor;
    let xsdt_len = tri!(
        build_xsdt(
            entries.split_at(entry_count).0,
            tail_mut(out, payload_offset + cursor)
        ),
        BlobError::Rsdp
    );
    signatures[index] = *b"XSDT";
    offsets[index] = cursor as u32;
    lengths[index] = xsdt_len as u32;
    index += 1;
    cursor = tri!(close_table(out, payload_offset, cursor, xsdt_len));

    // RSDP in the slot at payload offset 0 (the bytes after its 36 are already zero).
    tri!(
        build_rsdp(
            ACPI_REGION_BASE + xsdt_at as u64,
            tail_mut(out, payload_offset)
        ),
        BlobError::Rsdp
    );

    // `cursor` is 16-aligned here, so the FACS segment starts right after the payload.
    let payload_len = cursor;
    let facs_offset = payload_offset + payload_len;
    tri!(room(out, facs_offset + FACS_LEN));
    let facs_len = tri!(build_facs(tail_mut(out, facs_offset)), BlobError::Facs);
    let total_len = facs_offset + facs_len;
    // Anything past `total_len` is not part of the blob and is left alone.
    let header = head_mut(out, HEADER_LEN);
    copy_at(header, 0x00, &BLOB_MAGIC);
    copy_at(header, 0x08, &BLOB_VERSION.to_le_bytes());
    copy_at(header, 0x0C, &(HEADER_LEN as u32).to_le_bytes());
    copy_at(header, 0x10, &hash);
    copy_at(header, 0x30, &ACPI_REGION_BASE.to_le_bytes());
    copy_at(header, 0x38, &(payload_offset as u32).to_le_bytes());
    copy_at(header, 0x3C, &(payload_len as u32).to_le_bytes());
    copy_at(header, 0x40, &ACPI_REGION_BASE.to_le_bytes());
    copy_at(header, 0x48, &(index as u32).to_le_bytes());
    copy_at(header, 0x4C, &(total_len as u32).to_le_bytes());
    copy_at(header, 0x50, &(facs_offset as u32).to_le_bytes());
    copy_at(header, 0x54, &(facs_len as u32).to_le_bytes());
    copy_at(header, 0x58, &FACS_ADDRESS.to_le_bytes());
    let mut i = 0;
    while i < index {
        let at = HEADER_LEN + i * DIR_ENTRY_LEN;
        copy_at(out, at, &signatures[i]);
        copy_at(out, at + 4, &offsets[i].to_le_bytes());
        copy_at(out, at + 8, &lengths[i].to_le_bytes());
        i += 1;
    }
    Ok(total_len)
}

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

const fn mmio(device_type: u32, base: u64, length: u64) -> DeviceEntry {
    DeviceEntry {
        flags: FLAG_PRESENT,
        base,
        length,
        ..absent(device_type)
    }
}

/// The device table of the stage-4 board with `pci_ecam`, `acpi_pm` and `acpi_tables` set
/// (no NVMe, one screen), in the order of the handoff block (`board_devices` in `od-machine`,
/// `firmware_board.rs`). The machine test `acpi_blob::stage4_blob_is_built_for_this_boards_manifest`
/// recomputes the manifest hash from the real board configuration and compares it with the
/// header, so a drift of any value here is caught on the machine side.
pub const STAGE4_DEVICES: [DeviceEntry; 8] = [
    // COM1: 0x3F8, 8 ports, GSI 4, edge, COM number 1.
    DeviceEntry {
        flags: FLAG_PRESENT | FLAG_PIO,
        base: 0x3F8,
        length: 8,
        gsi: 4,
        param0: 1,
        ..absent(device_type::COM1)
    },
    mmio(device_type::LOCAL_APIC, 0xFEE0_0000, 0x1000),
    mmio(device_type::IOAPIC, 0xFEC0_0000, 0x1000),
    mmio(device_type::DISPLAY, 0xFC00_0000, 0x1000),
    absent(device_type::NVRAM),
    absent(device_type::TPM_CRB),
    // PCIe ECAM: segment 0, buses 0..=0, 1 MiB.
    mmio(device_type::PCIE_ECAM, 0xFC10_0000, 0x10_0000),
    // PM block 0x600..0x610, SCI GSI 9 level active-low; EVT 0x600, CNT 0x604, TMR 0x608,
    // RESET 0x60C; reset value 6, S5 SLP_TYP 5.
    DeviceEntry {
        flags: FLAG_PRESENT | FLAG_PIO,
        base: 0x600,
        length: 0x10,
        gsi: 9,
        irq_flags: 0b11,
        param0: 0x060C_0608_0604_0600,
        param1: 0x0506,
        ..absent(device_type::ACPI_PM)
    },
];

/// Upper bound of the stage-4 blob; the scratch buffer of the const evaluation.
pub const STAGE4_BLOB_CAPACITY: usize = 2048;

const STAGE4_BUILT: ([u8; STAGE4_BLOB_CAPACITY], usize) = {
    let mut out = [0u8; STAGE4_BLOB_CAPACITY];
    match build_blob(&STAGE4_DEVICES, &mut out) {
        Ok(len) => (out, len),
        Err(_) => panic!("build_blob failed in a const context"),
    }
};

/// Length of the stage-4 blob.
pub const STAGE4_BLOB_LEN: usize = STAGE4_BUILT.1;

/// The stage-4 blob as an exact-size array, for the static of the freestanding binary.
/// `N` must be [`STAGE4_BLOB_LEN`] (a compile-time error otherwise).
pub const fn stage4_blob_bytes<const N: usize>() -> [u8; N] {
    assert!(N == STAGE4_BUILT.1, "N must be STAGE4_BLOB_LEN");
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = STAGE4_BUILT.0[i];
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests;
