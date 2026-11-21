//! Slot and endpoint state of the xHCI model (`docs/specs/XHCI_DEVICE.md` sections 8 and 9.1):
//! the controller's own view; the driver's Output Device Context in guest RAM mirrors it.

/// Slot State (Slot Context DW3 31:27 uses 0 for Disabled and Enabled alike).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SlotState {
    Disabled,
    Enabled,
    Default,
    Addressed,
    Configured,
}

impl SlotState {
    /// The Slot State field of the Slot Context.
    pub(super) fn context_value(self) -> u32 {
        match self {
            Self::Disabled | Self::Enabled => 0,
            Self::Default => 1,
            Self::Addressed => 2,
            Self::Configured => 3,
        }
    }

    /// Default, Addressed or Configured: the slot has an Output Device Context in use.
    pub(super) fn is_addressable(self) -> bool {
        matches!(self, Self::Default | Self::Addressed | Self::Configured)
    }
}

/// Endpoint State (Endpoint Context DW0 2:0).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EndpointState {
    Disabled,
    Running,
    Halted,
    Stopped,
    /// Set by a TRB Error in a transfer ring (the transfer slice); Set TR Dequeue Pointer and
    /// the doorbell already know the state.
    #[cfg_attr(not(test), allow(dead_code))]
    Error,
}

impl EndpointState {
    pub(super) fn context_value(self) -> u32 {
        match self {
            Self::Disabled => 0,
            Self::Running => 1,
            Self::Halted => 2,
            Self::Stopped => 3,
            Self::Error => 4,
        }
    }
}

/// One endpoint the model supports: EP0 (DCI 1) or Interrupt IN (DCI 3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Endpoint {
    pub(super) state: EndpointState,
    /// TR Dequeue Pointer (16-byte aligned) and the dequeue cycle state.
    pub(super) dequeue: u64,
    pub(super) dcs: bool,
}

impl Endpoint {
    pub(super) const fn disabled() -> Self {
        Self {
            state: EndpointState::Disabled,
            dequeue: 0,
            dcs: false,
        }
    }
}

/// DCI of the control endpoint and of the Interrupt IN endpoint.
pub(super) const DCI_EP0: u32 = 1;
pub(super) const DCI_INTERRUPT_IN: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Slot {
    pub(super) state: SlotState,
    /// Root Hub Port Number from Address Device (1..=2); 0 before.
    pub(super) port: u8,
    pub(super) ep0: Endpoint,
    pub(super) interrupt_in: Endpoint,
    /// DCIs whose EP State changed outside a command (doorbell Stopped to Running) and has not
    /// been written to the Output Endpoint Context yet.
    pub(super) unmirrored: u32,
}

impl Slot {
    pub(super) const fn disabled() -> Self {
        Self {
            state: SlotState::Disabled,
            port: 0,
            ep0: Endpoint::disabled(),
            interrupt_in: Endpoint::disabled(),
            unmirrored: 0,
        }
    }

    /// Slot Not Enabled is the answer for a Disabled slot.
    pub(super) fn is_enabled(&self) -> bool {
        self.state != SlotState::Disabled
    }

    /// The endpoint with this DCI; every other DCI is Disabled and has no state here.
    pub(super) fn endpoint(&self, dci: u32) -> Option<&Endpoint> {
        match dci {
            DCI_EP0 => Some(&self.ep0),
            DCI_INTERRUPT_IN => Some(&self.interrupt_in),
            _ => None,
        }
    }

    pub(super) fn endpoint_mut(&mut self, dci: u32) -> Option<&mut Endpoint> {
        match dci {
            DCI_EP0 => Some(&mut self.ep0),
            DCI_INTERRUPT_IN => Some(&mut self.interrupt_in),
            _ => None,
        }
    }

    /// EP State of a DCI, Disabled for the ones the model does not have.
    pub(super) fn endpoint_state(&self, dci: u32) -> EndpointState {
        self.endpoint(dci)
            .map_or(EndpointState::Disabled, |endpoint| endpoint.state)
    }
}
