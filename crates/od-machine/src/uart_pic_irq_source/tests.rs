//! Tests of the UART THRE -> 8259 master IR4 source (S4-39i). The devices are driven through their
//! own `BusDevice` registers without a bus, the way the board loop does it: the guest writes the
//! registers, the loop pumps a THR byte to the COM1 transcript between quanta.

use super::*;
use crate::{
    AccessWidth, BusDevice, DeviceAccess, IoSpace, Pic8259, Pic8259Role, Pit8254Devices,
    Uart16550PollingTx, VirtualClockHandle,
};

const VECTOR: u8 = 0x30 + UART_IRQ_LINE;
const IR4: u8 = 1 << UART_IRQ_LINE;
const NONCE: &[u8] = b"OD_NONCE=s4-38l4a\n";

struct Rig {
    pic_device: Pic8259,
    pic: Pic8259Handle,
    uart_device: Uart16550PollingTx,
    uart: Uart16550Handle,
    source: UartPicIrqSource,
}

fn access(offset: u64) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Pio,
        offset,
        width: AccessWidth::Byte,
    }
}

impl Rig {
    /// A master initialised as Linux does with `imr`, a UART with 8N1 and a divisor (as the board
    /// configures COM1) and the MCR `mcr`.
    fn new(imr: u8, mcr: u8) -> Self {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let devices = Pit8254Devices::new(clock.clone());
        let (pic_device, pic) = Pic8259::new_with_handle(Pic8259Role::Master);
        let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
        let pit_pic = PitPicIrqSource::new(devices.handle.clone(), pic.clone(), clock);
        let source = UartPicIrqSource::new(pit_pic, uart.clone(), pic.clone());
        let mut rig = Self {
            pic_device,
            pic,
            uart_device,
            uart,
            source,
        };
        rig.pic_write(0, 0x11);
        rig.pic_write(1, 0x30);
        rig.pic_write(1, 0x04);
        rig.pic_write(1, 0x01);
        rig.pic_write(1, imr);
        rig.uart_write(3, 0x83);
        rig.uart_write(0, 0x01);
        rig.uart_write(1, 0x00);
        rig.uart_write(3, 0x03);
        rig.uart_write(4, mcr);
        rig
    }

    fn pic_write(&mut self, offset: u64, value: u8) {
        self.pic_device
            .write(access(offset), u64::from(value))
            .unwrap();
    }

    fn uart_write(&mut self, offset: u64, value: u8) {
        self.uart_device
            .write(access(offset), u64::from(value))
            .unwrap();
    }

    fn uart_read(&mut self, offset: u64) -> u8 {
        self.uart_device.read(access(offset)).unwrap() as u8
    }

    /// What the board loop does after a quantum: the THR byte goes to the transcript.
    fn pump(&self, transcript: &mut Vec<u8>) {
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                transcript.push(byte);
            }
        }
    }

    fn irr_isr(&self) -> (u8, u8) {
        let snapshot = self.pic.snapshot();
        (snapshot.irr, snapshot.isr)
    }
}

/// S4-24f: the 8259 source takes the line from the UART's port spec. A COM2 UART (IRQ3) raises
/// IR3 and the vector is ICW2 + 3; COM1 on IR4 is not touched.
#[test]
fn a_com2_uart_raises_ir3_and_not_ir4() {
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let devices = Pit8254Devices::new(clock.clone());
    let (mut pic_device, pic) = Pic8259::new_with_handle(Pic8259Role::Master);
    let (mut uart_device, uart) = Uart16550PollingTx::new_with_handle_at(UartPortSpec::COM2);
    assert_eq!(uart.port_spec(), UartPortSpec::COM2);
    let pit_pic = PitPicIrqSource::new(devices.handle.clone(), pic.clone(), clock);
    let source = UartPicIrqSource::new(pit_pic, uart.clone(), pic.clone());
    for (offset, value) in [(0, 0x11), (1, 0x30), (1, 0x04), (1, 0x01), (1, 0x00)] {
        pic_device.write(access(offset), value).unwrap();
    }
    // Enable RDA with OUT2 and inject one received byte: the level rises.
    uart_device.write(access(4), 0x08).unwrap();
    uart_device.write(access(1), 0x01).unwrap();
    assert_eq!(source.pending_vector(), None, "nothing before a byte");
    uart.inject_received_byte(b'a').unwrap();
    assert_eq!(source.pending_vector(), Some(0x30 + 3));
    assert_eq!(source.uart_edges(), 1);
    let snapshot = pic.snapshot();
    assert_eq!((snapshot.irr, snapshot.isr), (1 << 3, 0));
}

#[test]
fn the_uart_line_is_isa_irq_4() {
    assert_eq!(UART_IRQ_LINE, 4);
    assert_eq!(VECTOR, 0x34);
}

#[test]
fn ier_thre_with_an_empty_thr_raises_ir4_and_the_vector_is_icw2_plus_4() {
    let mut rig = Rig::new(0x00, 0x08);
    assert_eq!(rig.source.pending_vector(), None, "nothing before IER");
    rig.uart_write(1, 0x02);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    assert_eq!(rig.source.uart_edges(), 1);
    assert_eq!(rig.irr_isr(), (IR4, 0));
}

#[test]
fn without_ier_bit_1_there_is_no_irq_even_after_a_byte_was_sent() {
    let mut rig = Rig::new(0x00, 0x08);
    let mut transcript = Vec::new();
    // IER = 0, then IER = 1 (RDA only): no THRE source.
    for ier in [0x00, 0x01] {
        rig.uart_write(1, ier);
        rig.uart_write(0, b'A');
        rig.source.sample();
        rig.pump(&mut transcript);
        rig.source.sample();
        assert_eq!(rig.source.pending_vector(), None, "IER = {ier:#x}");
    }
    assert_eq!(transcript, b"AA");
    assert_eq!(rig.source.uart_edges(), 0);
    assert_eq!(rig.irr_isr(), (0, 0));
}

#[test]
fn the_mcr_out2_gate_keeps_the_line_low() {
    let mut rig = Rig::new(0x00, 0x00);
    rig.uart_write(1, 0x02);
    assert_eq!(rig.source.pending_vector(), None);
    assert_eq!(rig.source.uart_edges(), 0);
    // The condition is there (IIR shows it); OUT2 lets it out as an edge.
    assert_eq!(rig.uart.peek_iir(), 0x02);
    rig.uart_write(4, 0x08);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
}

#[test]
fn a_masked_ir4_latches_the_edge_and_the_unmask_delivers_it() {
    let mut rig = Rig::new(IR4, 0x08);
    rig.uart_write(1, 0x02);
    assert_eq!(rig.source.pending_vector(), None, "IR4 masked");
    assert_eq!(rig.irr_isr(), (IR4, 0));
    rig.pic_write(1, 0x00);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
}

#[test]
fn iir_shows_thre_and_the_read_acknowledges_it() {
    let mut rig = Rig::new(0x00, 0x08);
    rig.uart_write(1, 0x02);
    assert_eq!(rig.uart.peek_iir(), 0x02);
    assert_eq!(rig.uart_read(2), 0x02, "THRE source, bit 0 = 0");
    assert_eq!(rig.uart_read(2), 0x01, "acknowledged: no interrupt pending");
    assert!(!rig.uart.interrupt_line());
}

#[test]
fn a_high_level_makes_one_edge_and_a_thr_write_with_its_pump_makes_the_next() {
    let mut rig = Rig::new(0x00, 0x08);
    let mut transcript = Vec::new();
    rig.uart_write(1, 0x02);
    for _ in 0..3 {
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    }
    assert_eq!(rig.source.uart_edges(), 1, "the level stays high: one edge");
    // The handler: INTA, IIR read (clears THRE), EOI, then it writes a byte.
    assert!(rig.pic.acknowledge(VECTOR));
    assert_eq!(rig.irr_isr(), (0, IR4));
    assert_eq!(rig.uart_read(2), 0x02);
    rig.pic_write(0, 0x20);
    assert_eq!(rig.irr_isr(), (0, 0), "the EOI cleared ISR");
    assert_eq!(rig.source.pending_vector(), None);
    // The board loop samples before and after the pump.
    rig.uart_write(0, b'A');
    rig.source.sample();
    rig.pump(&mut transcript);
    rig.source.sample();
    assert_eq!(transcript, b"A");
    assert_eq!(rig.source.uart_edges(), 2);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    assert_eq!(rig.uart.peek_iir(), 0x02);
}

#[test]
fn a_thr_write_and_its_pump_without_an_iir_read_are_a_fall_and_a_rise_and_make_an_edge() {
    let mut rig = Rig::new(0x00, 0x08);
    let mut transcript = Vec::new();
    rig.uart_write(1, 0x02);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    assert!(rig.pic.acknowledge(VECTOR));
    rig.pic_write(0, 0x20);
    assert_eq!(rig.source.uart_edges(), 1);
    // The THR write drops the condition (no IIR read first); the pump raises it again.
    rig.uart_write(0, b'B');
    rig.source.sample();
    rig.pump(&mut transcript);
    rig.source.sample();
    assert_eq!(rig.source.uart_edges(), 2);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
}

#[test]
fn the_nonce_line_goes_through_thr_to_the_transcript_in_order_one_irq_per_byte() {
    let mut rig = Rig::new(0x00, 0x08);
    let mut transcript = Vec::new();
    rig.uart_write(1, 0x02);
    let mut delivered = 0;
    // The interrupt handler of the 8250 driver: INTA, IIR, one byte to THR (nothing when the
    // line is empty), EOI; the loop pumps after the THR write.
    let mut remaining = NONCE.iter();
    while let Some(vector) = rig.source.pending_vector() {
        assert_eq!(vector, VECTOR);
        assert!(rig.pic.acknowledge(vector));
        delivered += 1;
        assert_eq!(rig.uart_read(2), 0x02);
        rig.source.sample();
        if let Some(byte) = remaining.next() {
            rig.uart_write(0, *byte);
            rig.source.sample();
            rig.pump(&mut transcript);
            rig.source.sample();
        }
        rig.pic_write(0, 0x20);
        assert!(delivered <= NONCE.len() + 1, "the output never ends");
    }
    assert_eq!(transcript, NONCE);
    // One interrupt for the IER enable, one after each byte; the last one finds nothing to send.
    assert_eq!(delivered, NONCE.len() + 1);
    assert_eq!(rig.source.uart_edges(), NONCE.len() as u64 + 1);
    assert_eq!(rig.irr_isr(), (0, 0));
}

#[test]
fn the_pit_part_is_passed_through() {
    let rig = Rig::new(0x00, 0x08);
    assert_eq!(rig.source.next_deadline_ns(), None, "PIT not programmed");
    assert!(rig.source.pit_write_ends_quantum());
    assert_eq!(rig.source.irq0_edges(), 0);
    assert_eq!(rig.source.wake_ns(), None);
}

#[test]
fn a_halted_cpu_is_woken_by_a_pending_uart_interrupt() {
    let mut rig = Rig::new(0x00, 0x08);
    assert_eq!(rig.source.wake_ns(), None);
    rig.uart_write(1, 0x02);
    assert_eq!(rig.source.wake_ns(), Some(0), "deliverable now");
    assert_eq!(rig.source.uart_edges(), 1);
}

#[test]
fn rls_from_an_overrun_raises_ir4_and_the_lsr_read_drops_the_line() {
    // S4-24f: IER bit 2 alone (no RDA, no THRE) and an OE make one IR4 edge; reading LSR clears
    // the error bits and the condition; the next error is the next edge.
    let mut rig = Rig::new(0x00, 0x08);
    rig.uart_write(1, 0x04);
    assert_eq!(rig.uart.deliver_received_byte(0x41, 0), Ok(false));
    assert_eq!(
        rig.source.pending_vector(),
        None,
        "a received byte alone is no RLS"
    );
    assert_eq!(rig.uart.deliver_received_byte(0x42, 0), Ok(true));
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    assert_eq!(rig.source.uart_edges(), 1);
    assert_eq!(rig.irr_isr(), (IR4, 0));
    assert_eq!(rig.uart.peek_iir(), 0x06);
    // The RBR read does not clear RLS; the LSR read does.
    assert_eq!(rig.uart_read(0), 0x42);
    rig.source.sample();
    assert!(rig.uart.interrupt_line());
    assert_eq!(rig.uart_read(5) & 0x1e, 0x02);
    rig.source.sample();
    assert!(!rig.uart.interrupt_line());
    assert_eq!(rig.uart.peek_iir(), 0x01);
    assert_eq!(rig.uart.deliver_received_byte(0x43, 0x08), Ok(false));
    rig.source.sample();
    assert_eq!(
        rig.source.uart_edges(),
        2,
        "a framing error is the next edge"
    );
}

#[test]
fn rls_without_ier_bit_2_or_out2_makes_no_edge() {
    let mut rig = Rig::new(0x00, 0x00);
    rig.uart_write(1, 0x04);
    rig.uart.deliver_received_byte(0x01, 0x10).unwrap();
    rig.source.sample();
    assert_eq!(
        rig.uart.peek_iir(),
        0x06,
        "the condition is there, OUT2 holds the line"
    );
    assert_eq!(rig.source.uart_edges(), 0);
    rig.uart_write(4, 0x08);
    rig.source.sample();
    assert_eq!(rig.source.uart_edges(), 1);
    let mut quiet = Rig::new(0x00, 0x08);
    quiet.uart_write(1, 0x03);
    quiet.uart.deliver_received_byte(0x01, 0x10).unwrap();
    assert_eq!(quiet.uart.peek_iir(), 0x04, "IER bit 2 = 0: RDA, not RLS");
}
