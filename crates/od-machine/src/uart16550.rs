//! A bounded, synchronous UART register profile with single-byte RX and TX.
//!
//! This model accepts byte-wide PIO accesses only. Transmission advances only
//! when the caller explicitly invokes [`Uart16550PollingTx::transfer_holding_to_shift`]
//! and [`Uart16550PollingTx::complete_transmission`].
//!
//! S4-24e adds a bounded interrupt model: IER bit 0 (received data available)
//! and bit 1 (transmitter holding register empty), the IIR priority and clear
//! rules, and an MCR OUT2 gate for the interrupt output ([`Uart16550Handle::interrupt_line`]).
//! The line is a pure level; the session owner routes it to the IOAPIC.
//!
//! S4-38L7bq: the line control register (offset 3) stores and returns every
//! written bit, including DLAB (bit 7). With DLAB set, offsets 0 and 1 are the
//! divisor latch LSB and MSB (write and read); with DLAB clear they are
//! THR/RBR and IER. The divisor is only stored: the model has no baud rate
//! and the divisor never affects time. Semantics follow the 16550 datasheet
//! from memory and were not checked against a primary source. A THR write
//! still requires DLAB clear, LCR exactly 8N1 (`0x03`) and a non-zero divisor,
//! otherwise it is refused with `UnsupportedConfiguration` without mutation.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use std::sync::{Arc, Mutex};

const UART_REGISTER_COUNT: u64 = 8;
const LCR_DLAB: u8 = 0x80;
const LCR_8N1: u8 = 0x03;
const IIR_NO_INTERRUPT: u8 = 0x01;
const IIR_THRE: u8 = 0x02;
const IIR_RDA: u8 = 0x04;
const IER_RDA: u8 = 0x01;
const IER_THRE: u8 = 0x02;
const MCR_OUT2: u8 = 0x08;
const LSR_THRE: u8 = 0x20;
const LSR_TEMT: u8 = 0x40;

/// One synchronous UART polling device with a one-byte RX register and a
/// two-stage bounded TX path.
///
/// The device models a bounded subset of a 16550-style register layout. RX is
/// a single injected byte; there is no FIFO, interrupt delivery, or timing.
#[derive(Clone, Debug)]
pub struct Uart16550PollingTx {
    state: Arc<Mutex<UartState>>,
}

/// A safe control handle for the state owned by [`Uart16550PollingTx`].
///
/// Cloning this handle aliases the same device state. It does not create an
/// independent transmitter.
#[derive(Clone, Debug)]
pub struct Uart16550Handle {
    state: Arc<Mutex<UartState>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UartState {
    divisor: u16,
    line_control: u8,
    scratch: u8,
    holding: Option<u8>,
    shift: Option<u8>,
    received: Option<u8>,
    ier: u8,
    mcr: u8,
    thre_pending: bool,
}

impl Uart16550PollingTx {
    /// Creates a reset transmitter.
    pub fn new() -> Self {
        Self::with_shared_state()
    }

    /// Creates a transmitter and a handle that aliases its state.
    pub fn new_with_handle() -> (Self, Uart16550Handle) {
        let state = Arc::new(Mutex::new(UartState::new()));
        (
            Self {
                state: Arc::clone(&state),
            },
            Uart16550Handle { state },
        )
    }

    /// Returns a handle to this transmitter's state.
    pub fn handle(&self) -> Uart16550Handle {
        Uart16550Handle {
            state: Arc::clone(&self.state),
        }
    }

    fn with_shared_state() -> Self {
        Self {
            state: Arc::new(Mutex::new(UartState::new())),
        }
    }

    fn with_state<R>(&self, operation: impl FnOnce(&mut UartState) -> R) -> R {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation(&mut state)
    }

    fn snapshot(&self) -> UartState {
        self.with_state(|state| state.clone())
    }

    /// Moves the holding byte into an empty shift register.
    ///
    /// Returns `true` only when a byte was moved.
    pub fn transfer_holding_to_shift(&mut self) -> bool {
        self.with_state(UartState::transfer_holding_to_shift)
    }

    /// Completes the current shift-register byte and returns it unchanged.
    pub fn complete_transmission(&mut self) -> Option<u8> {
        self.with_state(UartState::complete_transmission)
    }

    /// Resets registers and discards any unfinished transmission.
    pub fn reset(&mut self) {
        self.with_state(UartState::reset);
    }

    /// Supplies one byte to the receive register. A full register is preserved
    /// and reports `Busy` until the guest reads the current byte.
    pub fn inject_received_byte(&mut self, value: u8) -> Result<(), DeviceError> {
        self.with_state(|state| state.inject_received_byte(value))
    }
}

impl Default for Uart16550PollingTx {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Uart16550PollingTx {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
    }
}

impl Eq for Uart16550PollingTx {}

impl Uart16550Handle {
    fn with_state<R>(&self, operation: impl FnOnce(&mut UartState) -> R) -> R {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation(&mut state)
    }

    /// Moves the shared device's holding byte into its shift register.
    pub fn transfer_holding_to_shift(&self) -> bool {
        self.with_state(UartState::transfer_holding_to_shift)
    }

    /// Whether the transmit holding register is occupied. A caller-side STOP
    /// predicate uses this to end a quantum right after a THR write
    /// (FIRMWARE_BOOT.md section 11).
    pub fn holding_occupied(&self) -> bool {
        self.with_state(|state| state.holding.is_some())
    }

    /// Completes and returns the shared device's current byte.
    pub fn complete_transmission(&self) -> Option<u8> {
        self.with_state(UartState::complete_transmission)
    }

    /// Resets the shared device and discards unfinished transmission state.
    pub fn reset(&self) {
        self.with_state(UartState::reset);
    }

    /// The UART interrupt output after the MCR OUT2 gate: a pending enabled
    /// condition (RDA or THRE) and OUT2 set. It has no side effects.
    pub fn interrupt_line(&self) -> bool {
        self.with_state(|state| state.interrupt_line())
    }

    /// The IIR value a guest read would return now, without clearing anything.
    pub fn peek_iir(&self) -> u8 {
        self.with_state(|state| state.iir())
    }

    /// The byte waiting in the receive register, without consuming it.
    pub fn received_byte(&self) -> Option<u8> {
        self.with_state(|state| state.received)
    }

    /// Supplies one byte to the receive register shared with the bus device.
    pub fn inject_received_byte(&self, value: u8) -> Result<(), DeviceError> {
        self.with_state(|state| state.inject_received_byte(value))
    }
}

impl UartState {
    const fn new() -> Self {
        Self {
            divisor: 0,
            line_control: 0,
            scratch: 0,
            holding: None,
            shift: None,
            received: None,
            ier: 0,
            mcr: 0,
            thre_pending: false,
        }
    }

    fn transfer_holding_to_shift(&mut self) -> bool {
        if self.shift.is_some() || self.holding.is_none() {
            return false;
        }
        self.shift = self.holding.take();
        // The holding register became empty: a THRE condition arises only
        // while the guest has it enabled.
        if self.ier & IER_THRE != 0 {
            self.thre_pending = true;
        }
        true
    }

    /// Highest-priority enabled condition: RDA over THRE; independent of OUT2.
    fn iir(&self) -> u8 {
        if self.ier & IER_RDA != 0 && self.received.is_some() {
            IIR_RDA
        } else if self.ier & IER_THRE != 0 && self.thre_pending {
            IIR_THRE
        } else {
            IIR_NO_INTERRUPT
        }
    }

    fn interrupt_line(&self) -> bool {
        self.mcr & MCR_OUT2 != 0 && self.iir() != IIR_NO_INTERRUPT
    }

    fn complete_transmission(&mut self) -> Option<u8> {
        self.shift.take()
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn inject_received_byte(&mut self, value: u8) -> Result<(), DeviceError> {
        if self.received.is_some() {
            return Err(DeviceError::Busy);
        }
        self.received = Some(value);
        Ok(())
    }

    fn dlab(&self) -> bool {
        self.line_control & LCR_DLAB != 0
    }

    fn line_configuration_ready(&self) -> bool {
        !self.dlab() && self.line_control == LCR_8N1 && self.divisor != 0
    }

    fn line_status(&self) -> u8 {
        let mut status = 0;
        if self.received.is_some() {
            status |= 0x01; // data ready
        }
        if self.holding.is_none() {
            status |= LSR_THRE;
        }
        if self.holding.is_none() && self.shift.is_none() {
            status |= LSR_TEMT;
        }
        status
    }

    fn validate_access(access: DeviceAccess) -> Result<u8, DeviceError> {
        if access.space != IoSpace::Pio {
            return Err(DeviceError::UnsupportedSpace);
        }
        if access.width != AccessWidth::Byte {
            return Err(DeviceError::UnsupportedWidth);
        }
        if access.offset >= UART_REGISTER_COUNT {
            return Err(DeviceError::UnknownOffset);
        }
        Ok(access.offset as u8)
    }

    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let offset = Self::validate_access(access)?;
        let value = match offset {
            0 if self.dlab() => self.divisor as u8,
            0 => {
                return self
                    .received
                    .take()
                    .map(u64::from)
                    .ok_or(DeviceError::WriteOnly);
            }
            1 if self.dlab() => (self.divisor >> 8) as u8,
            1 => self.ier,
            2 => {
                let iir = self.iir();
                // Reading IIR acknowledges a THRE interrupt only when THRE is
                // the reported source; RDA is cleared only by reading RBR.
                if iir == IIR_THRE {
                    self.thre_pending = false;
                }
                iir
            }
            3 => self.line_control,
            4 => self.mcr,
            5 => self.line_status(),
            7 => self.scratch,
            _ => return Err(DeviceError::UnknownOffset),
        };
        Ok(u64::from(value))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let offset = Self::validate_access(access)?;
        let value = value as u8;

        match offset {
            0 if self.dlab() => {
                self.divisor = (self.divisor & 0xff00) | u16::from(value);
            }
            0 => {
                if !self.line_configuration_ready() {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                if self.holding.is_some() {
                    return Err(DeviceError::Busy);
                }
                self.holding = Some(value);
                self.thre_pending = false;
            }
            1 if self.dlab() => {
                self.divisor = (self.divisor & 0x00ff) | (u16::from(value) << 8);
            }
            1 if value & !(IER_RDA | IER_THRE) == 0 => {
                // Enabling THRE while the holding register is empty raises
                // the condition; disabling it drops any pending condition.
                if value & IER_THRE == 0 {
                    self.thre_pending = false;
                } else if self.ier & IER_THRE == 0 && self.holding.is_none() {
                    self.thre_pending = true;
                }
                self.ier = value;
            }
            // Line-status and modem-status interrupts are not modelled.
            1 => return Err(DeviceError::UnsupportedConfiguration),
            2 if value == 0 => {}
            2 => return Err(DeviceError::UnsupportedConfiguration),
            // S4-38L7bq: LCR stores all eight bits (word length, stop, parity,
            // break, DLAB) and reads them back. Only the THR path interprets
            // them: a THR write still needs exactly 8N1 with DLAB clear.
            3 => self.line_control = value,
            4 if value & !MCR_OUT2 == 0 => self.mcr = value,
            // DTR/RTS/OUT1/loopback are not modelled.
            4 => return Err(DeviceError::UnsupportedConfiguration),
            5 => return Err(DeviceError::ReadOnly),
            7 => self.scratch = value,
            _ => return Err(DeviceError::UnknownOffset),
        }
        Ok(())
    }
}

impl BusDevice for Uart16550PollingTx {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.with_state(|state| state.read(access))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.with_state(|state| state.write(access, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(offset: u64) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width: AccessWidth::Byte,
        }
    }

    fn write(device: &mut Uart16550PollingTx, offset: u64, value: u8) {
        device.write(access(offset), u64::from(value)).unwrap();
    }

    fn setup_8n1_divisor(device: &mut Uart16550PollingTx) {
        write(device, 3, 0x83);
        write(device, 0, 0x34);
        write(device, 1, 0x12);
        write(device, 3, 0x03);
    }

    #[test]
    fn reset_status_is_empty_and_instance_state_is_isolated() {
        let mut first = Uart16550PollingTx::new();
        let mut second = Uart16550PollingTx::new();

        assert_eq!(first.read(access(5)), Ok(0x60));
        assert_eq!(second.read(access(5)), Ok(0x60));
        setup_8n1_divisor(&mut first);
        write(&mut first, 0, 0xff);
        assert_eq!(first.read(access(5)), Ok(0x00));
        assert_eq!(second.read(access(5)), Ok(0x60));
        assert_eq!(first.complete_transmission(), None);
        assert_eq!(second.complete_transmission(), None);
    }

    #[test]
    fn cloned_device_aliases_state_instead_of_copying_transmitter() {
        let mut device = Uart16550PollingTx::new();
        let mut alias = device.clone();
        setup_8n1_divisor(&mut device);
        write(&mut device, 0, 0xff);
        assert_eq!(alias.read(access(5)), Ok(0x00));
        alias.reset();
        assert_eq!(device.read(access(5)), Ok(0x60));
    }

    #[test]
    fn divisor_writes_do_not_emit_bytes_and_dlab_selects_registers() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x83);
        write(&mut device, 0, 0x34);
        write(&mut device, 1, 0x12);

        assert_eq!(device.read(access(0)), Ok(0x34));
        assert_eq!(device.read(access(1)), Ok(0x12));
        assert_eq!(device.complete_transmission(), None);
        assert_eq!(device.read(access(5)), Ok(0x60));

        write(&mut device, 3, 0x03);
        write(&mut device, 1, 0x00);
        write(&mut device, 2, 0x00);
        assert_eq!(device.read(access(1)), Ok(0));
        assert_eq!(device.read(access(2)), Ok(0x01));
        assert_eq!(device.read(access(3)), Ok(0x03));
    }

    #[test]
    fn holding_and_shift_registers_preserve_literal_bytes_and_status() {
        let mut device = Uart16550PollingTx::new();
        setup_8n1_divisor(&mut device);

        for (byte, status_during_shift) in [(0x00, 0x20), (0x0a, 0x20), (0xff, 0x20)] {
            write(&mut device, 0, byte);
            assert_eq!(device.read(access(5)), Ok(0x00));
            assert!(device.transfer_holding_to_shift());
            assert_eq!(device.read(access(5)), Ok(status_during_shift));
            assert!(!device.transfer_holding_to_shift());
            assert_eq!(device.complete_transmission(), Some(byte));
            assert_eq!(device.read(access(5)), Ok(0x60));
        }
    }

    #[test]
    fn busy_write_and_unsupported_configuration_are_atomic() {
        let mut device = Uart16550PollingTx::new();
        assert_eq!(
            device.write(access(0), 0x41),
            Err(DeviceError::UnsupportedConfiguration)
        );
        setup_8n1_divisor(&mut device);
        write(&mut device, 0, 0x41);
        assert_eq!(device.write(access(0), 0x42), Err(DeviceError::Busy));
        assert_eq!(device.read(access(5)), Ok(0x00));
        assert!(device.transfer_holding_to_shift());
        write(&mut device, 0, 0x42);
        assert_eq!(device.read(access(5)), Ok(0x00));
        assert_eq!(device.complete_transmission(), Some(0x41));
        assert_eq!(device.read(access(5)), Ok(0x00));
        assert!(device.transfer_holding_to_shift());
        assert_eq!(device.complete_transmission(), Some(0x42));
        assert_eq!(device.read(access(5)), Ok(0x60));

        assert_eq!(
            device.write(access(1), 0x04),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            device.write(access(2), 1),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(device.read(access(5)), Ok(0x60));
        assert_eq!(device.complete_transmission(), None);
    }

    #[test]
    fn reads_do_not_advance_transmission_and_reset_discards_pending_bytes() {
        let mut device = Uart16550PollingTx::new();
        setup_8n1_divisor(&mut device);
        write(&mut device, 0, 0x00);
        assert_eq!(device.read(access(5)), Ok(0x00));
        assert_eq!(device.read(access(2)), Ok(0x01));
        assert_eq!(device.read(access(3)), Ok(0x03));
        assert_eq!(device.read(access(7)), Ok(0));
        assert_eq!(device.read(access(5)), Ok(0x00));
        assert!(device.transfer_holding_to_shift());
        assert_eq!(device.read(access(5)), Ok(0x20));
        write(&mut device, 0, 0xff);
        assert_eq!(device.read(access(5)), Ok(0x00));
        assert_eq!(device.read(access(2)), Ok(0x01));
        device.reset();
        assert_eq!(device.read(access(5)), Ok(0x60));
        assert_eq!(device.complete_transmission(), None);
        assert_eq!(device.read(access(3)), Ok(0));
        assert_eq!(device.read(access(7)), Ok(0));
        assert_eq!(device.read(access(0)), Err(DeviceError::WriteOnly));
    }

    #[test]
    fn unsupported_spaces_widths_and_register_directions_are_explicit() {
        let mut device = Uart16550PollingTx::new();
        let mut unsupported_space = access(5);
        unsupported_space.space = IoSpace::Mmio;
        assert_eq!(
            device.read(unsupported_space),
            Err(DeviceError::UnsupportedSpace)
        );
        let mut unsupported_width = access(5);
        unsupported_width.width = AccessWidth::Word;
        assert_eq!(
            device.read(unsupported_width),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(device.read(access(8)), Err(DeviceError::UnknownOffset));
        assert_eq!(device.read(access(4)), Ok(0));
        assert_eq!(
            device.write(access(4), 0x01),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(device.write(access(5), 0x20), Err(DeviceError::ReadOnly));
        assert_eq!(device.read(access(0)), Err(DeviceError::WriteOnly));
        assert_eq!(device.read(access(6)), Err(DeviceError::UnknownOffset));
        assert_eq!(device.read(access(5)), Ok(0x60));
    }

    fn read(device: &mut Uart16550PollingTx, offset: u64) -> u8 {
        device.read(access(offset)).unwrap() as u8
    }

    #[test]
    fn ier_accepts_only_rda_and_thre_and_mcr_only_out2() {
        let mut device = Uart16550PollingTx::new();
        for value in [0x00, 0x01, 0x02, 0x03] {
            write(&mut device, 1, value);
            assert_eq!(read(&mut device, 1), value);
        }
        for value in [0x04_u8, 0x08, 0x0f, 0x80] {
            assert_eq!(
                device.write(access(1), u64::from(value)),
                Err(DeviceError::UnsupportedConfiguration)
            );
            assert_eq!(read(&mut device, 1), 0x03);
        }
        write(&mut device, 4, 0x08);
        assert_eq!(read(&mut device, 4), 0x08);
        for value in [0x01_u8, 0x02, 0x04, 0x10, 0x18] {
            assert_eq!(
                device.write(access(4), u64::from(value)),
                Err(DeviceError::UnsupportedConfiguration)
            );
            assert_eq!(read(&mut device, 4), 0x08);
        }
        // With DLAB set offset 1 is DLM and does not touch IER.
        write(&mut device, 3, 0x83);
        write(&mut device, 1, 0x00);
        write(&mut device, 3, 0x03);
        assert_eq!(read(&mut device, 1), 0x03);
    }

    #[test]
    fn rda_interrupt_requires_ier_and_clears_only_by_rbr_read() {
        let handle_owner = Uart16550PollingTx::new();
        let handle = handle_owner.handle();
        let mut device = handle_owner;
        write(&mut device, 4, 0x08);
        device.inject_received_byte(0x5a).unwrap();
        // IER = 0: the condition exists but is not reported or signalled.
        assert_eq!(read(&mut device, 2), 0x01);
        assert!(!handle.interrupt_line());
        write(&mut device, 1, 0x01);
        assert_eq!(handle.peek_iir(), 0x04);
        assert!(handle.interrupt_line());
        // Reading IIR is repeatable and does not clear RDA.
        assert_eq!(read(&mut device, 2), 0x04);
        assert_eq!(read(&mut device, 2), 0x04);
        assert!(handle.interrupt_line());
        assert_eq!(read(&mut device, 0), 0x5a);
        assert_eq!(read(&mut device, 2), 0x01);
        assert!(!handle.interrupt_line());
        // Disabling IER drops the line for a newly pending byte as well.
        device.inject_received_byte(0x11).unwrap();
        assert!(handle.interrupt_line());
        write(&mut device, 1, 0x00);
        assert!(!handle.interrupt_line());
        assert_eq!(read(&mut device, 2), 0x01);
    }

    #[test]
    fn out2_gates_only_the_line_not_the_iir() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        write(&mut device, 1, 0x01);
        device.inject_received_byte(0x01).unwrap();
        assert_eq!(read(&mut device, 2), 0x04);
        assert!(!handle.interrupt_line());
        write(&mut device, 4, 0x08);
        assert!(handle.interrupt_line());
        write(&mut device, 4, 0x00);
        assert!(!handle.interrupt_line());
        assert_eq!(handle.peek_iir(), 0x04);
    }

    #[test]
    fn thre_interrupt_raises_on_enable_and_after_transfer_and_clears_by_iir_or_thr() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        setup_8n1_divisor(&mut device);
        write(&mut device, 4, 0x08);
        assert!(!handle.interrupt_line());
        // Enabling THRE with an empty holding register raises it.
        write(&mut device, 1, 0x02);
        assert!(handle.interrupt_line());
        assert_eq!(read(&mut device, 2), 0x02);
        // IIR read acknowledged the THRE source.
        assert_eq!(read(&mut device, 2), 0x01);
        assert!(!handle.interrupt_line());
        // THR write clears nothing pending but arms the next empty edge.
        write(&mut device, 0, 0x41);
        assert!(!handle.interrupt_line());
        assert!(device.transfer_holding_to_shift());
        assert!(handle.interrupt_line());
        // A THR write also acknowledges a pending THRE condition.
        write(&mut device, 0, 0x42);
        assert!(!handle.interrupt_line());
        assert_eq!(read(&mut device, 2), 0x01);
        // Disabling THRE drops a pending condition.
        assert_eq!(device.complete_transmission(), Some(0x41));
        assert!(device.transfer_holding_to_shift());
        assert!(handle.interrupt_line());
        write(&mut device, 1, 0x00);
        assert!(!handle.interrupt_line());
    }

    #[test]
    fn rda_has_priority_over_thre_and_iir_read_keeps_rda() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        write(&mut device, 4, 0x08);
        write(&mut device, 1, 0x03);
        device.inject_received_byte(0x77).unwrap();
        assert_eq!(read(&mut device, 2), 0x04);
        // RDA reported, so the IIR read must not have acknowledged THRE.
        assert_eq!(read(&mut device, 0), 0x77);
        assert_eq!(read(&mut device, 2), 0x02);
        assert_eq!(read(&mut device, 2), 0x01);
        assert!(!handle.interrupt_line());
        device.reset();
        assert_eq!(read(&mut device, 1), 0);
        assert_eq!(read(&mut device, 4), 0);
        assert!(!handle.interrupt_line());
    }

    // S4-38L7bq. Oracles are hand-computed 16550 values (datasheet from
    // memory): DLAB is LCR bit 7 (0x80); 0x93 = DLAB | 0x13; divisor 0x000c.

    #[test]
    fn lcr_stores_all_bits_and_dlab_is_bit_7() {
        let mut device = Uart16550PollingTx::new();
        assert_eq!(read(&mut device, 3), 0x00);
        for value in [0x80_u8, 0x93, 0x13, 0x07, 0x3f, 0x7f, 0xff, 0x03, 0x00] {
            write(&mut device, 3, value);
            assert_eq!(read(&mut device, 3), value);
        }
        // 0x80 alone sets DLAB: offset 1 is then DLM, not IER.
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 1, 0x01);
        write(&mut device, 3, 0x80);
        assert_eq!(read(&mut device, 1), 0x00);
        write(&mut device, 3, 0x00);
        assert_eq!(read(&mut device, 1), 0x01);
    }

    #[test]
    fn dlab_divisor_latch_lsb_and_msb_round_trip() {
        let mut device = Uart16550PollingTx::new();
        // The Linux 0x93 write: DLAB | 0x13.
        write(&mut device, 3, 0x93);
        assert_eq!(read(&mut device, 3), 0x93);
        write(&mut device, 0, 0x0c);
        assert_eq!(read(&mut device, 0), 0x0c);
        assert_eq!(read(&mut device, 1), 0x00);
        write(&mut device, 1, 0x00);
        assert_eq!(read(&mut device, 1), 0x00);
        assert_eq!(read(&mut device, 0), 0x0c);
        write(&mut device, 1, 0xa5);
        assert_eq!(read(&mut device, 1), 0xa5);
        assert_eq!(read(&mut device, 0), 0x0c);
        write(&mut device, 0, 0xff);
        assert_eq!(read(&mut device, 0), 0xff);
        assert_eq!(read(&mut device, 1), 0xa5);
        // The latch is only stored: nothing is transmitted or received and
        // the line status is unchanged.
        assert_eq!(device.complete_transmission(), None);
        assert_eq!(read(&mut device, 5), 0x60);
    }

    #[test]
    fn dlab_clear_offsets_0_and_1_are_thr_rbr_and_ier() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x93);
        write(&mut device, 0, 0x0c);
        write(&mut device, 1, 0x00);
        write(&mut device, 3, 0x13);
        // IER is independent of the divisor MSB; RBR is empty (write-only
        // THR path unchanged).
        assert_eq!(read(&mut device, 1), 0x00);
        write(&mut device, 1, 0x03);
        assert_eq!(read(&mut device, 1), 0x03);
        assert_eq!(device.read(access(0)), Err(DeviceError::WriteOnly));
        device.inject_received_byte(0x5a).unwrap();
        assert_eq!(read(&mut device, 0), 0x5a);
        // The divisor survived the DLAB-clear accesses.
        write(&mut device, 3, 0x93);
        assert_eq!(read(&mut device, 0), 0x0c);
        assert_eq!(read(&mut device, 1), 0x00);
        // IER was not overwritten by DLM writes.
        write(&mut device, 3, 0x03);
        assert_eq!(read(&mut device, 1), 0x03);
    }

    #[test]
    fn thr_write_still_needs_exact_8n1_dlab_clear_and_nonzero_divisor() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x93);
        write(&mut device, 0, 0x0c);
        write(&mut device, 1, 0x00);
        // LCR 0x13 (parity-select bit set) and 0x07 are stored but not 8N1.
        for lcr in [0x13_u8, 0x07] {
            write(&mut device, 3, lcr);
            assert_eq!(
                device.write(access(0), 0x41),
                Err(DeviceError::UnsupportedConfiguration)
            );
            assert_eq!(read(&mut device, 5), 0x60);
            assert_eq!(read(&mut device, 3), lcr);
        }
        write(&mut device, 3, 0x03);
        write(&mut device, 0, 0x41);
        assert_eq!(read(&mut device, 5), 0x00);
        assert!(device.transfer_holding_to_shift());
        assert_eq!(device.complete_transmission(), Some(0x41));
    }

    #[test]
    fn dlab_write_to_unknown_offsets_and_other_refusals_do_not_mutate() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x93);
        write(&mut device, 0, 0x0c);
        write(&mut device, 1, 0x34);
        let before = device.snapshot();
        assert_eq!(device.write(access(6), 1), Err(DeviceError::UnknownOffset));
        assert_eq!(device.write(access(8), 1), Err(DeviceError::UnknownOffset));
        assert_eq!(device.read(access(6)), Err(DeviceError::UnknownOffset));
        assert_eq!(device.read(access(8)), Err(DeviceError::UnknownOffset));
        // FCR and MCR refusals are unchanged by the DLAB bit.
        assert_eq!(
            device.write(access(2), 0x07),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            device.write(access(4), 0x03),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(device.write(access(5), 0x00), Err(DeviceError::ReadOnly));
        assert_eq!(device.snapshot(), before);
    }
}
