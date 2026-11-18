//! PCI MSI-X capability, BAR table, and pending bits for the NVMe endpoint.

use super::controller::QUEUE_SLOTS;
use crate::{AccessWidth, DeviceError, LocalApicHandle};

pub(super) const CAPABILITY_OFFSET: u8 = 0x50;
pub(super) const TABLE_OFFSET: u64 = 0x2000;
const PBA_OFFSET: u64 = 0x3000;
const ENTRY_BYTES: u64 = 16;
const ENTRY_MASKED: u128 = 1 << 96;
const ENTRY_WRITABLE: u128 = ((ENTRY_MASKED << 1) - 1) & !3;
const ENABLE: u32 = 1 << 31;
const FUNCTION_MASK: u32 = 1 << 30;

#[derive(Debug)]
pub(super) struct Msix {
    control: u32,
    table: [u128; QUEUE_SLOTS],
    pending: u64,
    lapic: LocalApicHandle,
}

impl Msix {
    pub(super) fn new(lapic: LocalApicHandle) -> Self {
        Self {
            control: 0,
            table: [ENTRY_MASKED; QUEUE_SLOTS],
            pending: 0,
            lapic,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.control & ENABLE != 0
    }

    pub(super) fn config_read(&self, register: u16, width: AccessWidth) -> Option<u32> {
        const CAP: u16 = CAPABILITY_OFFSET as u16;
        const TABLE: u16 = CAP + 4;
        const PBA: u16 = CAP + 8;
        let dword = match register & !3 {
            CAP => self.control | ((QUEUE_SLOTS as u32 - 1) << 16) | 0x11,
            TABLE => TABLE_OFFSET as u32,
            PBA => PBA_OFFSET as u32,
            _ => return None,
        };
        let shift = (register & 3) * 8;
        Some((dword >> shift) & access_mask(width) as u32)
    }

    pub(super) fn write_control(&mut self, register: u16, width: AccessWidth, value: u32) {
        if register & !3 == u16::from(CAPABILITY_OFFSET) {
            let shift = (register & 3) * 8;
            let writable = ((access_mask(width) as u32) << shift) & (ENABLE | FUNCTION_MASK);
            self.control = (self.control & !writable) | ((value << shift) & writable);
        }
    }

    pub(super) fn read_bar(&self, offset: u64, width: AccessWidth) -> Result<u64, DeviceError> {
        check_alignment(offset, width)?;
        let mask = access_mask(width);
        if let Some((index, shift)) = table_location(offset) {
            return Ok((self.table[index] >> shift) as u64 & mask);
        }
        if (PBA_OFFSET..PBA_OFFSET + 8).contains(&offset) {
            return Ok((self.pending >> ((offset - PBA_OFFSET) * 8)) & mask);
        }
        Err(DeviceError::UnknownOffset)
    }

    pub(super) fn write_bar(
        &mut self,
        offset: u64,
        width: AccessWidth,
        value: u64,
    ) -> Result<(), DeviceError> {
        check_alignment(offset, width)?;
        if let Some((index, shift)) = table_location(offset) {
            let mask = u128::from(access_mask(width)) << shift;
            let writable = mask & ENTRY_WRITABLE;
            let entry = &mut self.table[index];
            *entry = (*entry & !writable) | ((u128::from(value) << shift) & writable);
            return Ok(());
        }
        if (PBA_OFFSET..PBA_OFFSET + 8).contains(&offset) {
            return Ok(()); // Pending bits are read-only.
        }
        Err(DeviceError::UnknownOffset)
    }

    pub(super) fn post(&mut self, vector: u16) {
        if self.enabled() {
            let vector = usize::from(vector);
            self.pending |= 1 << vector;
            self.try_deliver(vector);
        }
    }

    pub(super) fn deliver_pending(&mut self) {
        if self.enabled() && self.control & FUNCTION_MASK == 0 {
            let mut pending = self.pending;
            while pending != 0 {
                let vector = pending.trailing_zeros() as usize;
                self.try_deliver(vector);
                pending &= pending - 1;
            }
        }
    }

    fn try_deliver(&mut self, vector: usize) {
        let entry = self.table[vector];
        if self.control & FUNCTION_MASK != 0 || entry & ENTRY_MASKED != 0 {
            return;
        }
        let address = entry as u64;
        let data = (entry >> 64) as u32;
        // Fixed, physical xAPIC destination and edge trigger. Bit 14 of data
        // is the level field and is ignored for edge-triggered messages.
        if address & !0x000F_F000 != 0xFEE0_0000 || data & !0x40FF != 0 {
            return;
        }
        let destination = (address >> 12) as u8;
        if self.lapic.request_msi_edge(destination, data as u8).is_ok() {
            self.pending &= !(1 << vector);
        }
    }
}

fn table_location(offset: u64) -> Option<(usize, u64)> {
    let table_bytes = QUEUE_SLOTS as u64 * ENTRY_BYTES;
    if !(TABLE_OFFSET..TABLE_OFFSET + table_bytes).contains(&offset) {
        return None;
    }
    let offset = offset - TABLE_OFFSET;
    Some(((offset / ENTRY_BYTES) as usize, (offset % ENTRY_BYTES) * 8))
}

fn check_alignment(offset: u64, width: AccessWidth) -> Result<(), DeviceError> {
    if offset % u64::from(width.bytes()) != 0 {
        return Err(DeviceError::Misaligned);
    }
    Ok(())
}

fn access_mask(width: AccessWidth) -> u64 {
    u64::MAX >> (64 - width.bytes() * 8)
}
