//! The USB devices behind the root ports, as the controller sees them (`docs/specs/XHCI_DEVICE.md`
//! sections 9.3 and 10). The model of a device (S8-43: descriptors, HID reports) implements
//! [`UsbDevice`]; the controller only runs the transfer protocol: it hands over the setup packet
//! and OUT data of a control transfer, collects the response, and polls Interrupt IN reports.

/// What a device answers to a control request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlResponse {
    /// The request succeeded and has no data stage to return.
    Ack,
    /// The request succeeded and returns these bytes (IN data stage).
    Data(Vec<u8>),
    /// Request Error: the device answers STALL.
    Stall,
}

/// State of the Interrupt IN endpoint of a device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptStatus {
    /// No report is ready (the device would answer NAK).
    Nak,
    /// A report of this many bytes is ready.
    Report(usize),
    /// The endpoint is halted (SET_FEATURE ENDPOINT_HALT): the device answers STALL.
    Stall,
}

/// A device on a root port. Every method is called by the controller only; none may block.
pub trait UsbDevice: Send {
    /// The USB address the device answers to: 0 in the Default state.
    fn address(&self) -> u8;

    /// SET_ADDRESS, performed by the controller for Address Device with BSR = 0.
    fn set_address(&mut self, address: u8);

    /// Port reset, HCRST or Reset Device: back to the Default state (address 0, not configured,
    /// halt and queued reports cleared).
    fn reset(&mut self);

    /// A control transfer: the 8 setup bytes (bmRequestType, bRequest, wValue, wIndex, wLength)
    /// and the OUT data of the data stage (empty without one). SET_ADDRESS never arrives here.
    fn control(&mut self, setup: &[u8; 8], out_data: &[u8]) -> ControlResponse;

    /// The state of the Interrupt IN endpoint. Called again until it is not `Nak`.
    fn interrupt_in(&mut self) -> InterruptStatus;

    /// Takes the report announced by [`InterruptStatus::Report`]: exactly that many bytes.
    fn take_interrupt_report(&mut self) -> Vec<u8>;
}

/// The device of a port nobody attached a model to: it keeps an address, answers every control
/// request with STALL and never has a report.
#[derive(Debug, Default)]
pub(super) struct NullDevice {
    address: u8,
}

impl UsbDevice for NullDevice {
    fn address(&self) -> u8 {
        self.address
    }

    fn set_address(&mut self, address: u8) {
        self.address = address;
    }

    fn reset(&mut self) {
        self.address = 0;
    }

    fn control(&mut self, _setup: &[u8; 8], _out_data: &[u8]) -> ControlResponse {
        ControlResponse::Stall
    }

    fn interrupt_in(&mut self) -> InterruptStatus {
        InterruptStatus::Nak
    }

    fn take_interrupt_report(&mut self) -> Vec<u8> {
        Vec::new()
    }
}
