//! Command ring commands of the first slice: Enable Slot, Disable Slot, No Op
//! (`docs/specs/XHCI_DEVICE.md` section 9.1). Every other command type is answered
//! with TRB Error until the slices that implement it.

use super::regs::MAX_SLOTS;
use super::trb::{
    CC_NO_SLOTS_AVAILABLE, CC_SLOT_NOT_ENABLED, CC_SUCCESS, CC_TRB_ERROR, COMMAND_TYPES,
    TRB_TYPE_DISABLE_SLOT, TRB_TYPE_ENABLE_SLOT, TRB_TYPE_NO_OP_COMMAND, Trb,
};

/// Completion code and Slot ID of the Command Completion Event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommandResult {
    pub(super) code: u32,
    pub(super) slot_id: u32,
}

/// Slot table the commands work on: `enabled[n - 1]` is slot `n`.
pub(super) struct SlotTable<'a> {
    pub(super) enabled: &'a mut [bool; MAX_SLOTS],
    /// CONFIG.MaxSlotsEn.
    pub(super) max_slots_enabled: u8,
}

pub(super) fn execute(slots: SlotTable<'_>, trb: &Trb) -> CommandResult {
    match trb.trb_type() {
        TRB_TYPE_ENABLE_SLOT => enable_slot(slots, trb),
        TRB_TYPE_DISABLE_SLOT => disable_slot(slots, trb),
        TRB_TYPE_NO_OP_COMMAND => CommandResult {
            code: CC_SUCCESS,
            slot_id: 0,
        },
        // A command type the ring allows but the model does not implement keeps the
        // Slot ID of the TRB; a type not allowed in the command ring reports 0.
        kind if COMMAND_TYPES.contains(&kind) => CommandResult {
            code: CC_TRB_ERROR,
            slot_id: trb.slot_id(),
        },
        _ => CommandResult {
            code: CC_TRB_ERROR,
            slot_id: 0,
        },
    }
}

/// Enable Slot: the smallest free Slot ID in `1..=min(MaxSlotsEn, 8)`; the Slot Type
/// (DW3 20:16) must be 0, the only protocol type the model reports.
fn enable_slot(slots: SlotTable<'_>, trb: &Trb) -> CommandResult {
    if (trb.0[3] >> 16) & 0x1F != 0 {
        return CommandResult {
            code: CC_TRB_ERROR,
            slot_id: 0,
        };
    }
    let limit = usize::from(slots.max_slots_enabled).min(MAX_SLOTS);
    match slots.enabled[..limit].iter().position(|enabled| !enabled) {
        Some(index) => {
            slots.enabled[index] = true;
            CommandResult {
                code: CC_SUCCESS,
                slot_id: index as u32 + 1,
            }
        }
        None => CommandResult {
            code: CC_NO_SLOTS_AVAILABLE,
            slot_id: 0,
        },
    }
}

/// Disable Slot: a slot that is not enabled is Slot Not Enabled. DCBAA[n] is guest
/// memory and stays as the driver left it.
fn disable_slot(slots: SlotTable<'_>, trb: &Trb) -> CommandResult {
    let slot_id = trb.slot_id();
    let index = (slot_id as usize).checked_sub(1);
    match index.and_then(|index| slots.enabled.get_mut(index)) {
        Some(enabled) if *enabled => {
            *enabled = false;
            CommandResult {
                code: CC_SUCCESS,
                slot_id,
            }
        }
        _ => CommandResult {
            code: CC_SLOT_NOT_ENABLED,
            slot_id,
        },
    }
}
