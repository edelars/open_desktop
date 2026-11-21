//! TRB layout, TRB type codes and completion codes of the xHCI model
//! (`docs/specs/XHCI_DEVICE.md` sections 8 and 9).

/// A TRB is 16 bytes: parameter (DW0:DW1), status (DW2), control (DW3).
pub(super) const TRB_BYTES: u64 = 16;

pub(super) const TRB_TYPE_LINK: u32 = 6;
pub(super) const TRB_TYPE_ENABLE_SLOT: u32 = 9;
pub(super) const TRB_TYPE_DISABLE_SLOT: u32 = 10;
pub(super) const TRB_TYPE_NO_OP_COMMAND: u32 = 23;
pub(super) const TRB_TYPE_COMMAND_COMPLETION_EVENT: u32 = 33;
pub(super) const TRB_TYPE_PORT_STATUS_CHANGE_EVENT: u32 = 34;
pub(super) const TRB_TYPE_HOST_CONTROLLER_EVENT: u32 = 37;

/// Command TRB types that may appear in the command ring (xHCI table 6-91):
/// Enable Slot (9) through Set Extended Property (25).
pub(super) const COMMAND_TYPES: std::ops::RangeInclusive<u32> = 9..=25;

/// Completion codes (xHCI table 6-90) the model produces.
pub(super) const CC_SUCCESS: u32 = 1;
pub(super) const CC_TRB_ERROR: u32 = 5;
pub(super) const CC_NO_SLOTS_AVAILABLE: u32 = 9;
pub(super) const CC_SLOT_NOT_ENABLED: u32 = 11;
pub(super) const CC_EVENT_RING_FULL_ERROR: u32 = 21;

/// One TRB as four little-endian dwords.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Trb(pub(super) [u32; 4]);

impl Trb {
    pub(super) fn from_bytes(raw: &[u8; 16]) -> Self {
        let dword = |index: usize| {
            u32::from_le_bytes([
                raw[index * 4],
                raw[index * 4 + 1],
                raw[index * 4 + 2],
                raw[index * 4 + 3],
            ])
        };
        Self([dword(0), dword(1), dword(2), dword(3)])
    }

    pub(super) fn cycle(&self) -> bool {
        self.0[3] & 1 != 0
    }

    pub(super) fn toggle_cycle(&self) -> bool {
        self.0[3] & 2 != 0
    }

    pub(super) fn trb_type(&self) -> u32 {
        (self.0[3] >> 10) & 0x3F
    }

    pub(super) fn slot_id(&self) -> u32 {
        self.0[3] >> 24
    }

    /// DW0:DW1 as one 64-bit parameter.
    pub(super) fn parameter(&self) -> u64 {
        u64::from(self.0[0]) | (u64::from(self.0[1]) << 32)
    }
}

/// Command Completion Event (type 33): the address of the command TRB,
/// the completion code and the slot id; the cycle bit is added by the ring.
pub(super) fn command_completion_event(trb_address: u64, code: u32, slot_id: u32) -> Trb {
    Trb([
        trb_address as u32,
        (trb_address >> 32) as u32,
        code << 24,
        (slot_id << 24) | (TRB_TYPE_COMMAND_COMPLETION_EVENT << 10),
    ])
}

/// Port Status Change Event (type 34): Port ID in DW0 31:24, Success in DW2 31:24.
pub(super) fn port_status_change_event(port_id: u32) -> Trb {
    Trb([
        port_id << 24,
        0,
        CC_SUCCESS << 24,
        TRB_TYPE_PORT_STATUS_CHANGE_EVENT << 10,
    ])
}

/// Host Controller Event (type 37) with Event Ring Full Error (code 21).
pub(super) fn event_ring_full_event() -> Trb {
    Trb([
        0,
        0,
        CC_EVENT_RING_FULL_ERROR << 24,
        TRB_TYPE_HOST_CONTROLLER_EVENT << 10,
    ])
}
