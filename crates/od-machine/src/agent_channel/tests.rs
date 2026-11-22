//! Register-level tests of the agent channel device (S11-21): the COM2 UART driven through its own
//! `BusDevice` registers (no bus, no CPU), the host side through [`AgentChannel`].

use super::*;
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};

const LSR: u64 = 5;
const LSR_DR: u8 = 0x01;
const LSR_THRE: u8 = 0x20;
const LSR_TEMT: u8 = 0x40;

fn access(offset: u64) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Pio,
        offset,
        width: AccessWidth::Byte,
    }
}

struct Rig {
    device: Uart16550PollingTx,
    channel: AgentChannel,
}

impl Rig {
    /// A channel with the given queue capacities and the line set up as a driver does it: DLAB,
    /// divisor 1, 8N1.
    fn new(rx_capacity: usize, tx_capacity: usize) -> Self {
        let (device, channel) = AgentChannel::with_capacities(rx_capacity, tx_capacity);
        let mut rig = Self { device, channel };
        rig.out(3, 0x83);
        rig.out(0, 0x01);
        rig.out(1, 0x00);
        rig.out(3, 0x03);
        rig
    }

    fn out(&mut self, offset: u64, value: u8) {
        self.device
            .write(access(offset), u64::from(value))
            .unwrap_or_else(|error| panic!("write {offset}: {error:?}"));
    }

    fn inp(&mut self, offset: u64) -> u8 {
        self.device.read(access(offset)).unwrap() as u8
    }

    fn drain(&self) -> Vec<u8> {
        let mut out = vec![0u8; 64];
        let count = self.channel.read_from_guest(&mut out);
        out.truncate(count);
        out
    }

    /// What a polling guest does: read RBR while DR is set, with the owner pumping between reads.
    fn guest_reads_all(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.channel.pump();
        while self.inp(LSR) & LSR_DR != 0 {
            bytes.push(self.inp(0));
            self.channel.pump();
        }
        bytes
    }
}

#[test]
fn the_device_is_a_com2_uart_and_com1_keeps_its_placement() {
    let (device, channel) = AgentChannel::new();
    assert_eq!(device.port_spec(), UartPortSpec::COM2);
    assert_eq!(channel.uart().port_spec(), UartPortSpec::COM2);
    assert_eq!(
        (UartPortSpec::COM2.base_port, UartPortSpec::COM2.irq_line),
        (0x2f8, 3)
    );
    let (com1, handle) = Uart16550PollingTx::new_with_handle();
    assert_eq!(com1.port_spec(), UartPortSpec::COM1);
    assert_eq!(handle.port_spec(), UartPortSpec::COM1);
    assert_eq!(
        (UartPortSpec::COM1.base_port, UartPortSpec::COM1.irq_line),
        (0x3f8, 4)
    );
    assert_eq!(Uart16550PollingTx::new().port_spec(), UartPortSpec::COM1);
    assert_eq!(UartPortSpec::PORT_BYTES, 8);
}

#[test]
fn a_reset_channel_is_empty_and_ready_to_send() {
    let mut rig = Rig::new(8, 8);
    assert_eq!(rig.inp(LSR), LSR_THRE | LSR_TEMT);
    assert_eq!(
        rig.channel.stats(),
        AgentChannelStats {
            rx_capacity: 8,
            tx_capacity: 8,
            ..AgentChannelStats::default()
        }
    );
    assert!(!rig.channel.holding_occupied());
}

#[test]
fn a_guest_thr_byte_reaches_the_host_after_a_pump() {
    let mut rig = Rig::new(8, 8);
    rig.out(0, b'A');
    assert!(rig.channel.holding_occupied());
    assert_eq!(rig.inp(LSR) & (LSR_THRE | LSR_TEMT), 0);
    // A host read pumps first: the byte leaves THR and the transmitter is idle again.
    assert_eq!(rig.drain(), b"A");
    assert_eq!(rig.inp(LSR), LSR_THRE | LSR_TEMT);
    assert_eq!(rig.drain(), b"");
    let stats = rig.channel.stats();
    assert_eq!((stats.tx_bytes_accepted, stats.tx_bytes_read), (1, 1));
}

#[test]
fn a_second_thr_write_before_the_pump_is_busy() {
    let mut rig = Rig::new(8, 8);
    rig.out(0, 1);
    assert_eq!(rig.device.write(access(0), 2), Err(DeviceError::Busy));
    rig.channel.pump();
    rig.out(0, 2);
    assert_eq!(rig.drain(), [1, 2]);
}

#[test]
fn thr_is_refused_until_the_line_is_configured() {
    let (mut device, _channel) = AgentChannel::new();
    assert_eq!(
        device.write(access(0), 1),
        Err(DeviceError::UnsupportedConfiguration)
    );
}

#[test]
fn host_bytes_arrive_one_at_a_time_through_the_receive_register() {
    let mut rig = Rig::new(8, 8);
    assert_eq!(rig.channel.write_to_guest(b"abc"), 3);
    let stats = rig.channel.stats();
    assert_eq!((stats.rx_queued, stats.rx_bytes_delivered), (2, 1));
    assert_eq!(rig.inp(LSR) & LSR_DR, LSR_DR);
    assert_eq!(rig.inp(0), b'a');
    // The register stays empty until the owner pumps: DR is clear, no next byte yet.
    assert_eq!(rig.inp(LSR) & LSR_DR, 0);
    assert_eq!(rig.guest_reads_all(), b"bc");
    let stats = rig.channel.stats();
    assert_eq!(
        (
            stats.rx_queued,
            stats.rx_bytes_accepted,
            stats.rx_bytes_delivered
        ),
        (0, 3, 3)
    );
}

#[test]
fn the_rx_queue_is_bounded_and_the_overflow_is_counted() {
    let mut rig = Rig::new(4, 8);
    // Four fit; one of them moves into the receive register at once.
    assert_eq!(
        rig.channel.write_to_guest(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
        4
    );
    let stats = rig.channel.stats();
    assert_eq!((stats.rx_queued, stats.rx_overflow_bytes), (3, 6));
    // One slot is free again: one of three more fits.
    assert_eq!(rig.channel.write_to_guest(&[20, 21, 22]), 1);
    let stats = rig.channel.stats();
    assert_eq!(
        (
            stats.rx_queued,
            stats.rx_bytes_accepted,
            stats.rx_overflow_bytes
        ),
        (4, 5, 8)
    );
    // Nothing that was accepted is lost, and the order is kept.
    assert_eq!(rig.guest_reads_all(), [1, 2, 3, 4, 20]);
    assert_eq!(rig.channel.write_to_guest(&[]), 0);
}

#[test]
fn the_tx_queue_is_bounded_and_the_overflow_is_counted() {
    let mut rig = Rig::new(4, 2);
    for byte in 1..=5u8 {
        rig.out(0, byte);
        rig.channel.pump();
    }
    let stats = rig.channel.stats();
    assert_eq!(
        (
            stats.tx_queued,
            stats.tx_bytes_accepted,
            stats.tx_overflow_bytes
        ),
        (2, 2, 3)
    );
    // The first two are kept (a full queue drops the new byte), and the guest was never blocked.
    assert_eq!(rig.drain(), [1, 2]);
    assert_eq!(rig.inp(LSR), LSR_THRE | LSR_TEMT);
    rig.out(0, 6);
    assert_eq!(rig.drain(), [6]);
}

#[test]
fn a_host_read_takes_at_most_what_the_buffer_holds() {
    let rig = Rig::new(4, 8);
    assert_eq!(rig.channel.test_inject_from_guest(b"hello"), 5);
    let mut small = [0u8; 2];
    assert_eq!(rig.channel.read_from_guest(&mut small), 2);
    assert_eq!(&small, b"he");
    assert_eq!(rig.channel.read_from_guest(&mut []), 0);
    assert_eq!(rig.drain(), b"llo");
}

#[test]
fn test_injection_shares_the_tx_overflow_rule() {
    let rig = Rig::new(4, 3);
    assert_eq!(rig.channel.test_inject_from_guest(&[1, 2, 3, 4, 5]), 3);
    let stats = rig.channel.stats();
    assert_eq!(
        (
            stats.tx_queued,
            stats.tx_bytes_accepted,
            stats.tx_overflow_bytes
        ),
        (3, 3, 2)
    );
    assert_eq!(rig.drain(), [1, 2, 3]);
}

#[test]
fn a_reset_empties_the_queues_and_keeps_the_totals() {
    let mut rig = Rig::new(4, 4);
    rig.channel.write_to_guest(&[1, 2, 3]);
    rig.channel.test_inject_from_guest(&[9, 9]);
    rig.out(3, 0x1b);
    rig.channel.reset();
    let stats = rig.channel.stats();
    assert_eq!((stats.rx_queued, stats.tx_queued), (0, 0));
    assert_eq!((stats.rx_bytes_accepted, stats.tx_bytes_accepted), (3, 2));
    assert_eq!(rig.inp(3), 0, "the registers are back to power-on values");
    assert_eq!(rig.inp(LSR), LSR_THRE | LSR_TEMT);
}

#[test]
fn registers_follow_the_16450_profile_without_a_fifo() {
    let mut rig = Rig::new(4, 4);
    // FCR is accepted but the model announces no FIFO: IIR bits 7..6 stay 0 (a 16450).
    rig.out(2, 0xc7);
    assert_eq!(rig.inp(2), 0x01, "no interrupt, no FIFO flags");
    // SCR and LCR read back what was written.
    rig.out(7, 0xa5);
    assert_eq!(rig.inp(7), 0xa5);
    assert_eq!(rig.inp(3), 0x03);
    // IER keeps bits 3..0.
    rig.out(1, 0xff);
    assert_eq!(rig.inp(1), 0x0f);
}

#[test]
fn a_write_to_a_read_only_register_and_a_bad_access_are_device_errors() {
    let (mut device, _channel) = AgentChannel::new();
    assert_eq!(device.write(access(5), 0), Err(DeviceError::ReadOnly));
    assert_eq!(device.write(access(6), 0), Err(DeviceError::ReadOnly));
    // Loopback is not modelled (S4-38L7br), also on COM2.
    assert_eq!(
        device.write(access(4), 0x1a),
        Err(DeviceError::UnsupportedConfiguration)
    );
    let wide = DeviceAccess {
        width: AccessWidth::Word,
        ..access(5)
    };
    assert_eq!(device.read(wide), Err(DeviceError::UnsupportedWidth));
    assert_eq!(device.write(wide, 0), Err(DeviceError::UnsupportedWidth));
    let mmio = DeviceAccess {
        space: IoSpace::Mmio,
        ..access(5)
    };
    assert_eq!(device.read(mmio), Err(DeviceError::UnsupportedSpace));
    assert_eq!(device.read(access(8)), Err(DeviceError::UnknownOffset));
    assert_eq!(device.write(access(8), 0), Err(DeviceError::UnknownOffset));
}

#[test]
fn the_interrupt_output_is_a_level_that_follows_the_receive_register() {
    // The line is observable here; the board routes it to IOAPIC INTIN3 (see the firmware_board
    // test of the loop cycle).
    let mut rig = Rig::new(4, 4);
    rig.out(4, 0x08);
    rig.out(1, 0x01);
    assert!(!rig.channel.uart().interrupt_line());
    rig.channel.write_to_guest(b"x");
    assert!(rig.channel.uart().interrupt_line());
    assert_eq!(rig.inp(2), 0x04);
    rig.inp(0);
    assert!(!rig.channel.uart().interrupt_line());
}

#[test]
fn clones_share_the_queues_across_threads() {
    let rig = Rig::new(64, 64);
    let producer = rig.channel.clone();
    let handle = std::thread::spawn(move || {
        let mut sent = 0;
        while sent < 40 {
            sent += producer.write_to_guest(&[sent as u8]);
        }
    });
    handle.join().unwrap();
    assert_eq!(rig.channel.stats().rx_bytes_accepted, 40);
}
