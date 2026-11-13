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
//! still requires a non-zero divisor and an LCR that selects 8N1: bits 1..0 =
//! 11 (8 bits), bit 2 = 0 (one stop bit), bit 3 PEN = 0 (parity off), bit 6 = 0
//! (break off) and bit 7 DLAB = 0. Bits 4 (EPS) and 5 (stick parity) have no
//! effect while PEN = 0 and are ignored, so `0x13` passes. Otherwise the write
//! is refused with `UnsupportedConfiguration` without mutation.
//!
//! S4-38L9y: IER stores bits 3..0 (RDA, THRE, RLS, MS) and returns them;
//! bits 7..4 are dropped on write and read as 0 (a write of `0xff` reads back
//! `0x0f`). RLS and MS are stored only: the model has no line-status or
//! modem-status interrupt source, so they never change IIR or the line. RDA
//! and THRE behave as before. Semantics follow the 16550 datasheet from memory
//! and were not checked against a primary source.
//!
//! S4-38L9z: the FIFO control register (offset 2, write) accepts any byte and
//! stores it as written except bits 2..1 (RX and TX FIFO reset, self-clearing);
//! [`Uart16550Handle::fcr`] returns the stored value (the guest cannot read
//! it: offset 2 reads IIR). The model has no FIFO queues, so the reset bits are
//! no-ops and FCR never changes IIR or the interrupt line. IIR bits 7..6 stay
//! 0 ("FIFOs not enabled") even after FCR bit 0 = 1: announcing a 16550A would
//! switch the driver to FIFO mode that the one-byte model does not implement,
//! so the driver detects a 16450 (no FIFO) instead. RBR read with an empty
//! receiver returns the last byte received (0 after reset) and has no side
//! effect on RDA or the line. Semantics follow the 16550 datasheet from memory
//! and were not checked against a primary source.
//!
//! S4-38L7br: the modem control register (offset 4) stores bit 0 (DTR), bit 1
//! (RTS), bit 2 (OUT1) and bit 3 (OUT2) and reads them back. DTR, RTS and OUT1
//! have no effect: the board has no modem and the lines are not wired, so no
//! modem signal is modelled. OUT2 keeps its S4-24e role as the gate of the
//! interrupt line. Bit 4 (LOOP, loopback) and the reserved bits 5..7 are
//! refused with `UnsupportedConfiguration` without mutation: loopback would
//! change transmit and receive behaviour and is not modelled. Semantics
//! follow the 16550 datasheet from memory and were not checked against a
//! primary source.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use std::sync::{Arc, Mutex};

const UART_REGISTER_COUNT: u64 = 8;
const LCR_DLAB: u8 = 0x80;
const LCR_8N1: u8 = 0x03;
/// LCR bits that matter for an 8N1 transmit gate when PEN = 0: word length
/// (bits 1..0), stop (bit 2), PEN (bit 3), break (bit 6) and DLAB (bit 7).
/// EPS (bit 4) and stick parity (bit 5) are ignored while PEN = 0.
const LCR_TX_GATE_MASK: u8 = 0xcf;
const IIR_NO_INTERRUPT: u8 = 0x01;
const IIR_THRE: u8 = 0x02;
const IIR_RDA: u8 = 0x04;
const IER_RDA: u8 = 0x01;
const IER_THRE: u8 = 0x02;
/// IER bits 3..0 (RDA, THRE, RLS, MS) are stored; bits 7..4 read as 0. Only
/// RDA and THRE have interrupt sources; RLS (bit 2) and MS (bit 3) are
/// stored only.
const IER_STORED: u8 = 0x0f;
/// MCR bits 0..2 (DTR, RTS, OUT1) are stored and read back only; OUT2
/// (bit 3) additionally gates the interrupt line. LOOP (bit 4) and bits 5..7
/// are refused.
const MCR_STORED_ONLY: u8 = 0x07;
const MCR_OUT2: u8 = 0x08;
const MCR_ACCEPTED: u8 = MCR_STORED_ONLY | MCR_OUT2;
/// FCR bits 2..1 (RX and TX FIFO reset) are self-clearing and not stored.
const FCR_RESET_BITS: u8 = 0x06;
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
    /// Last byte that was received; RBR returns it while the receiver is empty.
    last_received: u8,
    /// FCR as written, without the self-clearing reset bits 2..1.
    fcr: u8,
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

    /// The FCR value as last written (bits 2..1 are self-clearing and read as
    /// 0). The guest cannot read it: offset 2 reads IIR.
    pub fn fcr(&self) -> u8 {
        self.with_state(|state| state.fcr)
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
            last_received: 0,
            fcr: 0,
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
        self.line_control & LCR_TX_GATE_MASK == LCR_8N1 && self.divisor != 0
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
                // A full receiver hands over its byte; an empty one keeps
                // showing the last received byte (S4-38L9z), without touching
                // RDA.
                if let Some(byte) = self.received.take() {
                    self.last_received = byte;
                }
                self.last_received
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
            1 => {
                // S4-38L9y: bits 3..0 (RDA, THRE, RLS, MS) are stored and read
                // back; bits 7..4 are not implemented and are dropped, so
                // they read as 0. RLS and MS are stored only: the model has
                // no line-status or modem-status interrupt source.
                let value = value & IER_STORED;
                // Enabling THRE while the holding register is empty raises
                // the condition; disabling it drops any pending condition.
                if value & IER_THRE == 0 {
                    self.thre_pending = false;
                } else if self.ier & IER_THRE == 0 && self.holding.is_none() {
                    self.thre_pending = true;
                }
                self.ier = value;
            }
            // S4-38L9z: FCR is stored (bit 0 enable, bits 7..6 trigger level);
            // the reset bits 2..1 have no queue to clear and do not stick.
            2 => self.fcr = value & !FCR_RESET_BITS,
            // S4-38L7bq: LCR stores all eight bits (word length, stop, parity,
            // break, DLAB) and reads them back. Only the THR path interprets
            // them: a THR write needs 8N1 with parity off and DLAB clear
            // (bits 4 and 5 are ignored while PEN = 0).
            3 => self.line_control = value,
            // S4-38L7br: DTR/RTS/OUT1 are stored only; OUT2 gates the line.
            4 if value & !MCR_ACCEPTED == 0 => self.mcr = value,
            // Loopback (bit 4) and reserved bits 5..7 are not modelled.
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

        // FCR = 1 (FIFO enable) is accepted since S4-38L9z (the IER = 0x04
        // refusal became execution in S4-38L9y); neither touches TX state.
        write(&mut device, 2, 1);
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
        assert_eq!(device.read(access(0)), Ok(0));
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
            device.write(access(4), 0x10),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(device.write(access(5), 0x20), Err(DeviceError::ReadOnly));
        assert_eq!(device.read(access(0)), Ok(0));
        assert_eq!(device.read(access(6)), Err(DeviceError::UnknownOffset));
        assert_eq!(device.read(access(5)), Ok(0x60));
    }

    fn read(device: &mut Uart16550PollingTx, offset: u64) -> u8 {
        device.read(access(offset)).unwrap() as u8
    }

    #[test]
    fn ier_stores_bits_0_to_3_and_mcr_accepts_only_bits_0_to_3() {
        let mut device = Uart16550PollingTx::new();
        for value in 0x00..=0x0f_u8 {
            write(&mut device, 1, value);
            assert_eq!(read(&mut device, 1), value);
        }
        write(&mut device, 1, 0x03);
        write(&mut device, 4, 0x08);
        assert_eq!(read(&mut device, 4), 0x08);
        for value in [0x10_u8, 0x18, 0x20, 0x40, 0x80, 0xff] {
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

    // S4-38L9y. Oracles are hand-computed 16550 values (datasheet from
    // memory): IER bits 3..0 = RDA, THRE, RLS, MS are stored; bits 7..4 read 0.

    #[test]
    fn ier_0x0f_is_stored_and_read_back_and_bits_7_to_4_read_as_zero() {
        let mut device = Uart16550PollingTx::new();
        // The kernel UART existence test: IER = 0, read, IER = 0x0f, read.
        write(&mut device, 1, 0x00);
        assert_eq!(read(&mut device, 1), 0x00);
        write(&mut device, 1, 0x0f);
        assert_eq!(read(&mut device, 1), 0x0f);
        write(&mut device, 1, 0x00);
        assert_eq!(read(&mut device, 1), 0x00);
        for (written, stored) in [
            (0xff_u8, 0x0f_u8),
            (0xf0, 0x00),
            (0x10, 0x00),
            (0x80, 0x00),
            (0xfd, 0x0d),
            (0x85, 0x05),
            (0x4a, 0x0a),
        ] {
            device.write(access(1), u64::from(written)).unwrap();
            assert_eq!(read(&mut device, 1), stored);
        }
    }

    #[test]
    fn ier_rls_and_ms_bits_are_stored_only_and_raise_nothing() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        setup_8n1_divisor(&mut device);
        write(&mut device, 4, 0x08);
        write(&mut device, 1, 0x0c);
        assert_eq!(read(&mut device, 1), 0x0c);
        assert_eq!(handle.peek_iir(), 0x01);
        assert!(!handle.interrupt_line());
        // Conditions exist but neither RLS nor MS reports them.
        device.inject_received_byte(0x33).unwrap();
        assert_eq!(handle.peek_iir(), 0x01);
        assert!(!handle.interrupt_line());
        assert_eq!(read(&mut device, 2), 0x01);
        assert_eq!(read(&mut device, 0), 0x33);
        write(&mut device, 0, 0x41);
        assert!(device.transfer_holding_to_shift());
        assert_eq!(handle.peek_iir(), 0x01);
        assert!(!handle.interrupt_line());
        assert_eq!(device.complete_transmission(), Some(0x41));
    }

    #[test]
    fn ier_0x0f_keeps_rda_and_thre_behaviour_of_bits_0_and_1() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        setup_8n1_divisor(&mut device);
        write(&mut device, 4, 0x08);
        // THRE enable edge with 0x0f raises exactly as with 0x02.
        write(&mut device, 1, 0x0f);
        assert_eq!(handle.peek_iir(), 0x02);
        assert!(handle.interrupt_line());
        assert_eq!(read(&mut device, 2), 0x02);
        assert_eq!(read(&mut device, 2), 0x01);
        // Rewriting 0x0f does not raise THRE again (no enable edge).
        write(&mut device, 1, 0x0f);
        assert_eq!(handle.peek_iir(), 0x01);
        // RDA has priority and is cleared only by an RBR read.
        device.inject_received_byte(0x5a).unwrap();
        assert_eq!(read(&mut device, 2), 0x04);
        assert_eq!(read(&mut device, 2), 0x04);
        assert_eq!(read(&mut device, 0), 0x5a);
        assert_eq!(read(&mut device, 2), 0x01);
        // Dropping bits 0 and 1 while keeping RLS and MS silences the line.
        write(&mut device, 0, 0x41);
        assert!(device.transfer_holding_to_shift());
        assert!(handle.interrupt_line());
        write(&mut device, 1, 0x0c);
        assert_eq!(handle.peek_iir(), 0x01);
        assert!(!handle.interrupt_line());
        // 0xff behaves as 0x0f for the enable edge.
        write(&mut device, 1, 0x00);
        assert_eq!(device.complete_transmission(), Some(0x41));
        write(&mut device, 1, 0xff);
        assert_eq!(read(&mut device, 1), 0x0f);
        assert_eq!(handle.peek_iir(), 0x02);
    }

    #[test]
    fn ier_write_does_not_touch_other_registers_and_dlab_redirects_offset_1() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x13);
        write(&mut device, 4, 0x0b);
        write(&mut device, 7, 0xa5);
        write(&mut device, 1, 0x0f);
        assert_eq!(read(&mut device, 3), 0x13);
        assert_eq!(read(&mut device, 4), 0x0b);
        assert_eq!(read(&mut device, 7), 0xa5);
        assert_eq!(read(&mut device, 5), 0x60);
        // Other writes do not change IER either.
        write(&mut device, 3, 0x1b);
        write(&mut device, 4, 0x01);
        write(&mut device, 7, 0x5a);
        assert_eq!(read(&mut device, 1), 0x0f);
        // With DLAB set offset 1 is DLM: a 0x0f / 0xff write there is a
        // divisor byte (not masked to 4 bits) and IER keeps 0x0f.
        write(&mut device, 3, 0x83);
        write(&mut device, 1, 0xff);
        assert_eq!(read(&mut device, 1), 0xff);
        write(&mut device, 1, 0x00);
        assert_eq!(read(&mut device, 1), 0x00);
        write(&mut device, 3, 0x03);
        assert_eq!(read(&mut device, 1), 0x0f);
        // DLAB set: a DLM write of 0x00 does not clear a stored IER, and an
        // IER write with DLAB clear does not touch the divisor.
        write(&mut device, 3, 0x83);
        write(&mut device, 0, 0x0c);
        write(&mut device, 1, 0x12);
        write(&mut device, 3, 0x03);
        write(&mut device, 1, 0x05);
        write(&mut device, 3, 0x83);
        assert_eq!(read(&mut device, 0), 0x0c);
        assert_eq!(read(&mut device, 1), 0x12);
        write(&mut device, 3, 0x03);
        assert_eq!(read(&mut device, 1), 0x05);
        // Reset clears the IER.
        device.reset();
        assert_eq!(read(&mut device, 1), 0x00);
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
        assert_eq!(device.read(access(0)), Ok(0));
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
    fn thr_write_refuses_non_8n1_lcr_and_zero_divisor_without_mutation() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x93);
        write(&mut device, 0, 0x0c);
        write(&mut device, 1, 0x00);
        // 0x07: two stop bits; 0x0b/0x1b/0x2b: parity enabled; 0x43: break;
        // 0x02/0x01/0x00: not 8 data bits.
        for lcr in [0x07_u8, 0x0b, 0x1b, 0x2b, 0x43, 0x02, 0x01, 0x00] {
            write(&mut device, 3, lcr);
            let before = device.snapshot();
            assert_eq!(
                device.write(access(0), 0x41),
                Err(DeviceError::UnsupportedConfiguration)
            );
            assert_eq!(device.snapshot(), before);
            assert_eq!(read(&mut device, 5), 0x60);
            assert_eq!(read(&mut device, 3), lcr);
        }
        // Zero divisor with a valid LCR is refused too.
        let mut zero = Uart16550PollingTx::new();
        write(&mut zero, 3, 0x03);
        let before = zero.snapshot();
        assert_eq!(
            zero.write(access(0), 0x41),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(zero.snapshot(), before);
    }

    #[test]
    fn thr_write_accepts_lcr_0x13_and_ignores_eps_and_stick_bits_when_pen_is_clear() {
        // 0x03 reference: THR write fills holding, LSR goes 0x60 -> 0x00.
        let mut reference = Uart16550PollingTx::new();
        write(&mut reference, 3, 0x83);
        write(&mut reference, 0, 0x0c);
        write(&mut reference, 1, 0x00);
        write(&mut reference, 3, 0x03);
        write(&mut reference, 0, 0x41);
        assert_eq!(read(&mut reference, 5), 0x00);

        // 0x13: EPS set, PEN clear. 0x23: stick set, PEN clear. 0x33: both.
        for lcr in [0x13_u8, 0x23, 0x33] {
            let mut device = Uart16550PollingTx::new();
            write(&mut device, 3, 0x80 | lcr);
            write(&mut device, 0, 0x0c);
            write(&mut device, 1, 0x00);
            write(&mut device, 3, lcr);
            assert_eq!(read(&mut device, 5), 0x60);
            write(&mut device, 0, 0x41);
            assert_eq!(read(&mut device, 5), 0x00);
            assert_eq!(read(&mut device, 3), lcr);
            assert!(device.transfer_holding_to_shift());
            assert_eq!(read(&mut device, 5), 0x20);
            assert_eq!(device.complete_transmission(), Some(0x41));
            assert_eq!(read(&mut device, 5), 0x60);
        }
    }

    #[test]
    fn dlab_set_never_reaches_thr_even_with_an_8n1_word() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x93);
        write(&mut device, 0, 0x41); // divisor LSB, not THR
        write(&mut device, 1, 0x00);
        assert_eq!(read(&mut device, 5), 0x60);
        assert_eq!(device.complete_transmission(), None);
        assert!(!device.transfer_holding_to_shift());
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
        // The MCR refusal is unchanged by the DLAB bit (FCR = 0x07 is
        // accepted since S4-38L9z and moved to the FCR tests).
        assert_eq!(
            device.write(access(4), 0x10),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(device.write(access(5), 0x00), Err(DeviceError::ReadOnly));
        assert_eq!(device.snapshot(), before);
    }

    // S4-38L7br. Oracles are hand-computed 16550 values (datasheet from
    // memory): MCR bit 0 DTR = 0x01, bit 1 RTS = 0x02, bit 2 OUT1 = 0x04,
    // bit 3 OUT2 = 0x08, bit 4 LOOP = 0x10, bits 5..7 reserved.

    #[test]
    fn mcr_stores_and_reads_back_dtr_rts_out1_out2() {
        let mut device = Uart16550PollingTx::new();
        assert_eq!(read(&mut device, 4), 0x00);
        // 0x01 is the Linux 0xffffffff81fee68f write (DTR).
        for value in [
            0x01_u8, 0x02, 0x03, 0x04, 0x07, 0x08, 0x09, 0x0b, 0x0f, 0x00,
        ] {
            write(&mut device, 4, value);
            assert_eq!(read(&mut device, 4), value);
        }
    }

    #[test]
    fn mcr_loopback_and_reserved_bits_are_refused_without_mutation() {
        for stored in [0x00_u8, 0x01, 0x0f] {
            let mut device = Uart16550PollingTx::new();
            write(&mut device, 4, stored);
            let before = device.snapshot();
            for value in [0x10_u8, 0x11, 0x1f, 0x20, 0x40, 0x80, 0x81, 0xff] {
                assert_eq!(
                    device.write(access(4), u64::from(value)),
                    Err(DeviceError::UnsupportedConfiguration)
                );
                assert_eq!(device.snapshot(), before);
                assert_eq!(read(&mut device, 4), stored);
            }
        }
    }

    #[test]
    fn mcr_dtr_rts_out1_do_not_gate_the_line_or_touch_other_registers() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        write(&mut device, 3, 0x93);
        write(&mut device, 0, 0x0c);
        write(&mut device, 1, 0x00);
        write(&mut device, 3, 0x03);
        write(&mut device, 1, 0x01);
        device.inject_received_byte(0x5a).unwrap();
        assert!(!handle.interrupt_line());
        // Without OUT2 the line stays low; the whole state changes only in
        // the mcr field.
        for value in [0x01_u8, 0x02, 0x04, 0x07] {
            let before = device.snapshot();
            write(&mut device, 4, value);
            let mut expected = before.clone();
            expected.mcr = value;
            assert_eq!(device.snapshot(), expected);
            assert!(!handle.interrupt_line());
            assert_eq!(handle.peek_iir(), 0x04);
            assert_eq!(read(&mut device, 4), value);
        }
        // OUT2 together with DTR/RTS/OUT1 raises the line as OUT2 alone did.
        write(&mut device, 4, 0x0f);
        assert!(handle.interrupt_line());
        write(&mut device, 4, 0x07);
        assert!(!handle.interrupt_line());
        // The divisor and THR path are intact.
        write(&mut device, 3, 0x83);
        assert_eq!(read(&mut device, 0), 0x0c);
        write(&mut device, 3, 0x03);
        assert_eq!(read(&mut device, 0), 0x5a);
        write(&mut device, 0, 0x41);
        assert!(device.transfer_holding_to_shift());
        assert_eq!(device.complete_transmission(), Some(0x41));
    }

    #[test]
    fn mcr_bits_0_to_2_do_not_gate_a_pending_thre_but_out2_does() {
        let mut device = Uart16550PollingTx::new();
        let handle = device.handle();
        setup_8n1_divisor(&mut device);
        // Enabling THRE with an empty holding register raises the condition.
        write(&mut device, 1, 0x02);
        assert!(device.snapshot().thre_pending);
        assert_eq!(handle.peek_iir(), 0x02);
        for value in [0x01_u8, 0x02, 0x04, 0x07] {
            write(&mut device, 4, value);
            assert!(!handle.interrupt_line());
            assert!(device.snapshot().thre_pending);
        }
        // OUT2 raises the line as before, alone or with the stored bits.
        for value in [0x08_u8, 0x09, 0x0f] {
            write(&mut device, 4, 0x00);
            assert!(!handle.interrupt_line());
            write(&mut device, 4, value);
            assert!(handle.interrupt_line());
        }
        write(&mut device, 4, 0x07);
        assert!(!handle.interrupt_line());
    }

    #[test]
    fn mcr_is_the_same_register_with_dlab_set() {
        let mut device = Uart16550PollingTx::new();
        write(&mut device, 3, 0x93);
        for value in 0x01_u8..=0x07 {
            let before = device.snapshot();
            write(&mut device, 4, value);
            let mut expected = before.clone();
            expected.mcr = value;
            assert_eq!(device.snapshot(), expected);
            assert_eq!(read(&mut device, 4), value);
        }
        let before = device.snapshot();
        assert_eq!(
            device.write(access(4), 0x10),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(device.snapshot(), before);
        assert_eq!(read(&mut device, 4), 0x07);
        assert_eq!(read(&mut device, 3), 0x93);
    }

    // S4-38L9z. Oracles are hand-computed 16550 values (datasheet from memory).
    #[test]
    fn fcr_accepts_any_byte_and_stores_it_without_the_reset_bits() {
        let (mut device, handle) = Uart16550PollingTx::new_with_handle();
        assert_eq!(handle.fcr(), 0);
        write(&mut device, 2, 0x01);
        assert_eq!(handle.fcr(), 0x01);
        write(&mut device, 2, 0xc7);
        assert_eq!(handle.fcr(), 0xc1);
        for value in 0..=0xff_u8 {
            write(&mut device, 2, value);
            assert_eq!(handle.fcr(), value & !0x06);
        }
        write(&mut device, 2, 0x00);
        assert_eq!(handle.fcr(), 0);
        // FCR is the same under DLAB.
        write(&mut device, 3, 0x80);
        write(&mut device, 2, 0x01);
        assert_eq!(handle.fcr(), 0x01);
    }

    #[test]
    fn fcr_does_not_change_iir_or_the_interrupt_line() {
        let (mut device, handle) = Uart16550PollingTx::new_with_handle();
        write(&mut device, 4, 0x08);
        write(&mut device, 2, 0x01);
        assert_eq!(read(&mut device, 2), 0x01);
        assert_eq!(read(&mut device, 2) & 0xc0, 0);
        assert!(!handle.interrupt_line());
        // RDA pending: FCR leaves IIR, the line and the received byte alone.
        write(&mut device, 1, 0x01);
        device.inject_received_byte(0x41).unwrap();
        assert!(handle.interrupt_line());
        write(&mut device, 2, 0xc7);
        assert_eq!(read(&mut device, 2), 0x04);
        assert!(handle.interrupt_line());
        assert_eq!(handle.received_byte(), Some(0x41));
        assert_eq!(read(&mut device, 5) & 0x01, 0x01);
        // THRE pending: reading IIR after FCR still acknowledges it once.
        assert_eq!(read(&mut device, 0), 0x41);
        write(&mut device, 1, 0x02);
        assert_eq!(read(&mut device, 2), 0x02);
        write(&mut device, 2, 0x01);
        assert_eq!(read(&mut device, 2), 0x01);
        assert!(!handle.interrupt_line());
    }

    #[test]
    fn fcr_reset_bits_leave_lcr_mcr_ier_scratch_and_rx_alone() {
        let (mut device, handle) = Uart16550PollingTx::new_with_handle();
        write(&mut device, 3, 0x13);
        write(&mut device, 4, 0x0b);
        write(&mut device, 1, 0x05);
        write(&mut device, 7, 0xa5);
        device.inject_received_byte(0x33).unwrap();
        let before = device.snapshot();
        write(&mut device, 2, 0x07);
        assert_eq!(read(&mut device, 3), 0x13);
        assert_eq!(read(&mut device, 4), 0x0b);
        assert_eq!(read(&mut device, 1), 0x05);
        assert_eq!(read(&mut device, 7), 0xa5);
        assert_eq!(handle.received_byte(), Some(0x33));
        let mut after = device.snapshot();
        assert_eq!(after.fcr, 0x01);
        after.fcr = before.fcr;
        assert_eq!(after, before);
    }

    #[test]
    fn kernel_probe_sequence_after_fcr_reads_as_a_16450() {
        // autoconfig: LCR = 0xbf, offset 2 = 0 (EFR absent: FCR), LCR = 0,
        // FCR = 1, IIR[7:6] = 0, SCR test, RBR read, IER clear.
        let (mut device, handle) = Uart16550PollingTx::new_with_handle();
        write(&mut device, 3, 0xbf);
        write(&mut device, 2, 0x00);
        write(&mut device, 3, 0x00);
        write(&mut device, 2, 0x01);
        assert_eq!(read(&mut device, 2) & 0xc0, 0);
        write(&mut device, 7, 0xa5);
        assert_eq!(read(&mut device, 7), 0xa5);
        write(&mut device, 7, 0x5a);
        assert_eq!(read(&mut device, 7), 0x5a);
        write(&mut device, 7, 0);
        assert_eq!(read(&mut device, 0), 0);
        write(&mut device, 1, 0);
        assert_eq!(read(&mut device, 1), 0);
        assert_eq!(read(&mut device, 5), 0x60);
        assert!(!handle.interrupt_line());
    }

    #[test]
    fn rbr_on_empty_receiver_returns_last_byte_without_touching_rda() {
        let (mut device, handle) = Uart16550PollingTx::new_with_handle();
        assert_eq!(read(&mut device, 0), 0);
        assert_eq!(read(&mut device, 5), 0x60);
        device.inject_received_byte(0x7e).unwrap();
        assert_eq!(read(&mut device, 5), 0x61);
        // A full receiver still returns the byte once and clears RDA.
        assert_eq!(read(&mut device, 0), 0x7e);
        assert_eq!(read(&mut device, 5), 0x60);
        // Empty again: the last byte repeats, no RDA, injection still works.
        assert_eq!(read(&mut device, 0), 0x7e);
        assert_eq!(read(&mut device, 0), 0x7e);
        assert_eq!(read(&mut device, 5), 0x60);
        assert_eq!(handle.received_byte(), None);
        write(&mut device, 4, 0x08);
        write(&mut device, 1, 0x01);
        assert!(!handle.interrupt_line());
        assert_eq!(read(&mut device, 2), 0x01);
        device.inject_received_byte(0x01).unwrap();
        assert_eq!(read(&mut device, 2), 0x04);
        assert_eq!(read(&mut device, 0), 0x01);
        // Reset forgets the last byte.
        device.reset();
        assert_eq!(read(&mut device, 0), 0);
        assert_eq!(handle.fcr(), 0);
    }
}
