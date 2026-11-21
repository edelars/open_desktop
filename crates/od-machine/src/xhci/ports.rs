//! Root hub ports of the xHCI model: two USB 2.0 ports with a Full-speed device
//! permanently attached (`docs/specs/XHCI_DEVICE.md` section 6).

const PORTSC_CCS: u32 = 1;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PLS_SHIFT: u32 = 5;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_PRC: u32 = 1 << 21;

/// PLS values the model reports: U0 once the port is enabled, Polling otherwise.
const PLS_U0: u32 = 0;
const PLS_POLLING: u32 = 7;
/// Port Speed ID 1: Full-speed (xHCI table 7-13, PSIC = 0).
const SPEED_FULL: u32 = 1;

/// State of one port. CCS is always 1; PEC, WRC, OCC, PLC and CEC are never set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Port {
    ped: bool,
    pr: bool,
    reset_done_ns: u64,
    csc: bool,
    prc: bool,
    event_reported: bool,
    /// USB address of the attached device: 0 (Default state) until SET_ADDRESS.
    device_address: u8,
}

impl Port {
    /// Attached, not reset: PED = 0, Polling, connect change not yet reported.
    pub(super) fn new() -> Self {
        Self {
            ped: false,
            pr: false,
            reset_done_ns: 0,
            csc: true,
            prc: false,
            event_reported: false,
            device_address: 0,
        }
    }

    pub(super) fn portsc(&self) -> u32 {
        let pls = if self.ped && !self.pr {
            PLS_U0
        } else {
            PLS_POLLING
        };
        PORTSC_CCS
            | if self.ped { PORTSC_PED } else { 0 }
            | if self.pr { PORTSC_PR } else { 0 }
            | (pls << PORTSC_PLS_SHIFT)
            | PORTSC_PP
            | (SPEED_FULL << PORTSC_SPEED_SHIFT)
            | if self.csc { PORTSC_CSC } else { 0 }
            | if self.prc { PORTSC_PRC } else { 0 }
    }

    /// The port is enabled (PED = 1; a reset in progress clears PED).
    pub(super) fn is_enabled(&self) -> bool {
        self.ped
    }

    pub(super) fn device_address(&self) -> u8 {
        self.device_address
    }

    /// SET_ADDRESS sent to the device by Address Device.
    pub(super) fn set_device_address(&mut self, address: u8) {
        self.device_address = address;
    }

    /// Any change bit (CSC, PRC) is set.
    pub(super) fn has_change(&self) -> bool {
        self.csc || self.prc
    }

    /// A Port Status Change Event is owed: change bits are set and no event was
    /// published since they last went from clear to set (xHCI 4.19.2, PSCEG).
    pub(super) fn event_due(&self) -> bool {
        self.has_change() && !self.event_reported
    }

    pub(super) fn mark_event_reported(&mut self) {
        self.event_reported = true;
    }

    /// Completes a port reset whose time has come. Returns `true` when PRC went
    /// from 0 to 1 (USBSTS.PCD).
    pub(super) fn poll(&mut self, now_ns: u64) -> bool {
        if !self.pr || now_ns < self.reset_done_ns {
            return false;
        }
        self.pr = false;
        self.ped = true;
        // A port reset returns the device to the Default state (address 0).
        self.device_address = 0;
        let newly_changed = !self.prc;
        self.prc = true;
        newly_changed
    }

    /// A PORTSC write: change bits are RW1C, PED = 1 disables the port, PR = 1
    /// starts a reset of `reset_ns` virtual nanoseconds. Everything else is
    /// read-only or not implemented. Returns `true` when the write completed a
    /// zero-length reset with a new PRC.
    pub(super) fn write(&mut self, value: u32, now_ns: u64, reset_ns: u64) -> bool {
        if value & PORTSC_CSC != 0 {
            self.csc = false;
        }
        if value & PORTSC_PRC != 0 {
            self.prc = false;
        }
        if !self.has_change() {
            self.event_reported = false;
        }
        if value & PORTSC_PED != 0 && self.ped && !self.pr {
            self.ped = false;
        }
        if value & PORTSC_PR != 0 && !self.pr {
            self.pr = true;
            self.ped = false;
            self.reset_done_ns = now_ns.saturating_add(reset_ns);
        }
        self.poll(now_ns)
    }
}
