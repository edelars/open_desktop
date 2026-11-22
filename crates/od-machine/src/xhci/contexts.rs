//! Context layout in guest RAM (CSZ = 0: 32-byte contexts; `docs/specs/XHCI_DEVICE.md`
//! section 8) and the DMA access to them. All DMA goes through [`GuestDma`].

use crate::nvme::{DmaError, GuestDma};

pub(super) const CONTEXT_BYTES: u64 = 32;

/// A Slot, Endpoint or Input Control Context as eight dwords.
pub(super) type Context = [u32; 8];

pub(super) fn read_context(dma: &mut dyn GuestDma, address: u64) -> Result<Context, DmaError> {
    let mut raw = [0u8; CONTEXT_BYTES as usize];
    dma.read(address, &mut raw)?;
    let mut context = [0u32; 8];
    for (dword, chunk) in context.iter_mut().zip(raw.chunks_exact(4)) {
        *dword = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    Ok(context)
}

pub(super) fn write_context(
    dma: &mut dyn GuestDma,
    address: u64,
    context: &Context,
) -> Result<(), DmaError> {
    let mut raw = [0u8; CONTEXT_BYTES as usize];
    for (chunk, dword) in raw.chunks_exact_mut(4).zip(context) {
        chunk.copy_from_slice(&dword.to_le_bytes());
    }
    dma.write(address, &raw)
}

/// Input Context: the Input Control Context, then the Slot Context, then Endpoint Contexts
/// by DCI (DCI n at index n + 1).
pub(super) const fn input_slot(base: u64) -> u64 {
    base + CONTEXT_BYTES
}

pub(super) const fn input_endpoint(base: u64, dci: u32) -> u64 {
    base + CONTEXT_BYTES * (dci as u64 + 1)
}

/// Output Device Context: the Slot Context, then Endpoint Contexts by DCI (DCI n at index n).
pub(super) const fn output_slot(base: u64) -> u64 {
    base
}

pub(super) const fn output_endpoint(base: u64, dci: u32) -> u64 {
    base + CONTEXT_BYTES * dci as u64
}

/// Input Control Context: drop flags (DW0) and add flags (DW1).
pub(super) fn drop_flags(control: &Context) -> u32 {
    control[0]
}

pub(super) fn add_flags(control: &Context) -> u32 {
    control[1]
}

pub(super) mod slot {
    use super::Context;

    pub(in super::super) fn route_string(context: &Context) -> u32 {
        context[0] & 0xF_FFFF
    }

    pub(in super::super) fn context_entries(context: &Context) -> u32 {
        context[0] >> 27
    }

    pub(in super::super) fn set_context_entries(context: &mut Context, entries: u32) {
        context[0] = (context[0] & 0x07FF_FFFF) | (entries << 27);
    }

    pub(in super::super) fn max_exit_latency(context: &Context) -> u32 {
        context[1] & 0xFFFF
    }

    pub(in super::super) fn set_max_exit_latency(context: &mut Context, latency: u32) {
        context[1] = (context[1] & !0xFFFF) | latency;
    }

    pub(in super::super) fn root_hub_port(context: &Context) -> u32 {
        (context[1] >> 16) & 0xFF
    }

    pub(in super::super) fn interrupter_target(context: &Context) -> u32 {
        context[2] >> 22
    }

    pub(in super::super) fn set_interrupter_target(context: &mut Context, target: u32) {
        context[2] = (context[2] & 0x003F_FFFF) | (target << 22);
    }

    pub(in super::super) fn set_device_address(context: &mut Context, address: u32) {
        context[3] = (context[3] & !0xFF) | address;
    }

    pub(in super::super) fn set_slot_state(context: &mut Context, state: u32) {
        context[3] = (context[3] & 0x07FF_FFFF) | (state << 27);
    }
}

pub(super) mod endpoint {
    use super::Context;

    pub(in super::super) fn set_state(context: &mut Context, state: u32) {
        context[0] = (context[0] & !0x7) | state;
    }

    /// Mult (9:8) and MaxPStreams (14:10): both must be 0.
    pub(in super::super) fn mult_and_streams(context: &Context) -> u32 {
        (context[0] >> 8) & 0x7F
    }

    pub(in super::super) fn state(context: &Context) -> u32 {
        context[0] & 0x7
    }

    /// Interval (23:16): a power of two of 125 microsecond units.
    pub(in super::super) fn interval(context: &Context) -> u32 {
        (context[0] >> 16) & 0xFF
    }

    pub(in super::super) fn endpoint_type(context: &Context) -> u32 {
        (context[1] >> 3) & 0x7
    }

    pub(in super::super) fn max_burst(context: &Context) -> u32 {
        (context[1] >> 8) & 0xFF
    }

    pub(in super::super) fn max_packet_size(context: &Context) -> u32 {
        context[1] >> 16
    }

    pub(in super::super) fn set_max_packet_size(context: &mut Context, size: u32) {
        context[1] = (context[1] & 0xFFFF) | (size << 16);
    }

    /// DW2:DW3 as written: TR Dequeue Pointer (63:4), SCT/reserved (3:1) and DCS (0).
    pub(in super::super) fn dequeue_raw(context: &Context) -> u64 {
        u64::from(context[2]) | (u64::from(context[3]) << 32)
    }

    pub(in super::super) fn set_dequeue(context: &mut Context, pointer: u64, dcs: bool) {
        let raw = (pointer & !0xF) | u64::from(dcs);
        context[2] = raw as u32;
        context[3] = (raw >> 32) as u32;
    }
}
