//! TCG ACPI 1.50 §3.2.4, revision 6 TPM2 table for a device-resident CRB.

use super::bytes::{copy_at, fill_range, head_mut, tri};
use super::manifest::DeviceEntry;
use super::table::{TableError, finish_checksum, write_header};

pub(super) const SIGNATURE: [u8; 4] = *b"TPM2";
const LEN: usize = 52;

pub(super) const fn build_tpm2(entry: &DeviceEntry, out: &mut [u8]) -> Result<usize, TableError> {
    if out.len() < LEN {
        return Err(TableError::TooShort);
    }
    let table = head_mut(out, LEN);
    fill_range(table, 0, LEN, 0);
    tri!(write_header(table, SIGNATURE, 6, *b"ODFWTPM2", LEN as u32));
    // Platform Class 0 (client), no start method parameters or ACPI log area pointer.
    copy_at(table, 40, &(entry.base + 0x40).to_le_bytes());
    copy_at(table, 48, &7u32.to_le_bytes());
    tri!(finish_checksum(table));
    Ok(LEN)
}
