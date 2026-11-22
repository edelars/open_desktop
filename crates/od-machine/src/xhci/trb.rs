//! TRB layout, TRB type codes and completion codes of the xHCI model
//! (`docs/specs/XHCI_DEVICE.md` sections 8 and 9).

/// A TRB is 16 bytes: parameter (DW0:DW1), status (DW2), control (DW3).
pub(super) const TRB_BYTES: u64 = 16;

pub(super) const TRB_TYPE_NORMAL: u32 = 1;
pub(super) const TRB_TYPE_SETUP_STAGE: u32 = 2;
pub(super) const TRB_TYPE_DATA_STAGE: u32 = 3;
pub(super) const TRB_TYPE_STATUS_STAGE: u32 = 4;
pub(super) const TRB_TYPE_LINK: u32 = 6;
pub(super) const TRB_TYPE_ENABLE_SLOT: u32 = 9;
pub(super) const TRB_TYPE_DISABLE_SLOT: u32 = 10;
pub(super) const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
pub(super) const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
pub(super) const TRB_TYPE_EVALUATE_CONTEXT: u32 = 13;
pub(super) const TRB_TYPE_RESET_ENDPOINT: u32 = 14;
pub(super) const TRB_TYPE_STOP_ENDPOINT: u32 = 15;
pub(super) const TRB_TYPE_SET_TR_DEQUEUE_POINTER: u32 = 16;
pub(super) const TRB_TYPE_RESET_DEVICE: u32 = 17;
pub(super) const TRB_TYPE_NO_OP_COMMAND: u32 = 23;
pub(super) const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
pub(super) const TRB_TYPE_COMMAND_COMPLETION_EVENT: u32 = 33;
pub(super) const TRB_TYPE_PORT_STATUS_CHANGE_EVENT: u32 = 34;
pub(super) const TRB_TYPE_HOST_CONTROLLER_EVENT: u32 = 37;

/// Command TRB types that may appear in the command ring (xHCI table 6-91):
/// Enable Slot (9) through Set Extended Property (25).
pub(super) const COMMAND_TYPES: std::ops::RangeInclusive<u32> = 9..=25;

/// Completion codes (xHCI table 6-90) the model produces.
pub(super) const CC_SUCCESS: u32 = 1;
pub(super) const CC_USB_TRANSACTION_ERROR: u32 = 4;
pub(super) const CC_DATA_BUFFER_ERROR: u32 = 2;
pub(super) const CC_BABBLE_DETECTED_ERROR: u32 = 3;
pub(super) const CC_TRB_ERROR: u32 = 5;
pub(super) const CC_STALL_ERROR: u32 = 6;
pub(super) const CC_NO_SLOTS_AVAILABLE: u32 = 9;
pub(super) const CC_SHORT_PACKET: u32 = 13;
pub(super) const CC_SLOT_NOT_ENABLED: u32 = 11;
pub(super) const CC_PARAMETER_ERROR: u32 = 17;
pub(super) const CC_CONTEXT_STATE_ERROR: u32 = 19;
pub(super) const CC_EVENT_RING_FULL_ERROR: u32 = 21;
pub(super) const CC_STOPPED: u32 = 26;
pub(super) const CC_STOPPED_LENGTH_INVALID: u32 = 27;

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

    /// Interrupt on Short Packet (DW3 bit 2).
    pub(super) fn isp(&self) -> bool {
        self.0[3] & (1 << 2) != 0
    }

    /// Chain bit (DW3 bit 4).
    pub(super) fn chain(&self) -> bool {
        self.0[3] & (1 << 4) != 0
    }

    /// Interrupt On Completion (DW3 bit 5).
    pub(super) fn ioc(&self) -> bool {
        self.0[3] & (1 << 5) != 0
    }

    /// Immediate Data (DW3 bit 6).
    pub(super) fn idt(&self) -> bool {
        self.0[3] & (1 << 6) != 0
    }

    /// Block Event Interrupt (DW3 bit 9).
    pub(super) fn bei(&self) -> bool {
        self.0[3] & (1 << 9) != 0
    }

    /// Data Stage and Status Stage direction: IN (DW3 bit 16).
    pub(super) fn dir_in(&self) -> bool {
        self.0[3] & (1 << 16) != 0
    }

    /// TRB Transfer Length (DW2 16:0).
    pub(super) fn length(&self) -> u32 {
        self.0[2] & 0x1_FFFF
    }

    /// Interrupter Target (DW2 31:22): the model has only Interrupter 0.
    pub(super) fn interrupter_target(&self) -> u32 {
        self.0[2] >> 22
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

/// Transfer Event (type 32): the TRB the event is about, the completion code and the number of
/// bytes not transferred (23:0), the slot and endpoint ids.
pub(super) fn transfer_event(
    trb_address: u64,
    code: u32,
    remaining: u32,
    slot_id: u32,
    endpoint_id: u32,
) -> Trb {
    Trb([
        trb_address as u32,
        (trb_address >> 32) as u32,
        (code << 24) | (remaining & 0xFF_FFFF),
        (slot_id << 24) | (endpoint_id << 16) | (TRB_TYPE_TRANSFER_EVENT << 10),
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
