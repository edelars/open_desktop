//! Tests of the transfer rings (S8-42, slice C): control transfers on EP0 and Interrupt IN, with
//! a scripted USB device behind the root port. Every refusal of the contract has its test.

use super::*;
use crate::xhci::slots::EndpointState;
use crate::xhci::trb::{
    CC_BABBLE_DETECTED_ERROR, CC_DATA_BUFFER_ERROR, CC_SHORT_PACKET, CC_STALL_ERROR, CC_STOPPED,
    CC_USB_TRANSACTION_ERROR, TRB_TYPE_LINK, TRB_TYPE_RESET_ENDPOINT,
    TRB_TYPE_SET_TR_DEQUEUE_POINTER, TRB_TYPE_STOP_ENDPOINT, TRB_TYPE_TRANSFER_EVENT,
};
use crate::{ControlResponse, InterruptStatus, UsbDevice};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const IOC: u32 = 1 << 5;
const ISP: u32 = 1 << 2;
const CH: u32 = 1 << 4;
const BEI: u32 = 1 << 9;
const IDT: u32 = 1 << 6;

const EP0_RING: u64 = 0x7_0000;
const INTERRUPT_RING: u64 = 0x7_1000;
const BUF: u64 = 0x9_0000;
const BUF2: u64 = 0xA_0000;
const BIG_RING: u64 = 0xB_0000;
const INPUT: u64 = 0x5_0000;
const OUTPUT_BASE: u64 = 0x6_0000;
const EP0: u32 = 1;
const EP3: u32 = 3;

#[derive(Default)]
struct Script {
    address: u8,
    requests: Vec<([u8; 8], Vec<u8>)>,
    responses: VecDeque<ControlResponse>,
    reports: VecDeque<Vec<u8>>,
    halted: bool,
    resets: u32,
    /// Announce this length instead of the real report length.
    announce: Option<usize>,
}

/// A device whose answers the test scripts; clones share the script.
#[derive(Clone, Default)]
struct TestDevice(Arc<Mutex<Script>>);

impl TestDevice {
    fn with<R>(&self, f: impl FnOnce(&mut Script) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }
    fn respond(&self, response: ControlResponse) {
        self.with(|s| s.responses.push_back(response));
    }
    fn report(&self, report: &[u8]) {
        self.with(|s| s.reports.push_back(report.to_vec()));
    }
    fn requests(&self) -> Vec<([u8; 8], Vec<u8>)> {
        self.with(|s| s.requests.clone())
    }
}

impl UsbDevice for TestDevice {
    fn address(&self) -> u8 {
        self.with(|s| s.address)
    }
    fn set_address(&mut self, address: u8) {
        self.with(|s| s.address = address);
    }
    fn reset(&mut self) {
        self.with(|s| {
            s.address = 0;
            s.resets += 1;
        });
    }
    fn control(&mut self, setup: &[u8; 8], out_data: &[u8]) -> ControlResponse {
        self.with(|s| {
            s.requests.push((*setup, out_data.to_vec()));
            s.responses.pop_front().unwrap_or(ControlResponse::Stall)
        })
    }
    fn interrupt_in(&mut self) -> InterruptStatus {
        self.with(|s| {
            if s.halted {
                InterruptStatus::Stall
            } else {
                s.reports.front().map_or(InterruptStatus::Nak, |r| {
                    InterruptStatus::Report(s.announce.unwrap_or(r.len()))
                })
            }
        })
    }
    fn take_interrupt_report(&mut self) -> Vec<u8> {
        self.with(|s| s.reports.pop_front().unwrap_or_default())
    }
}

fn output(slot: u32) -> u64 {
    OUTPUT_BASE + u64::from(slot) * 0x1000
}

fn slot_ctx(port: u32, entries: u32) -> [u32; 8] {
    [entries << 27, port << 16, 0, 0, 0, 0, 0, 0]
}

fn ep_ctx(kind: u32, mps: u32, dequeue: u64, interval: u32) -> [u32; 8] {
    let raw = dequeue | 1;
    [
        interval << 16,
        (kind << 3) | (3 << 1) | (mps << 16),
        raw as u32,
        (raw >> 32) as u32,
        8,
        0,
        0,
        0,
    ]
}

fn pointer_command(kind: u32, pointer: u64, slot: u32, flags: u32) -> [u32; 4] {
    [
        pointer as u32,
        (pointer >> 32) as u32,
        0,
        (slot << 24) | (kind << 10) | flags,
    ]
}

fn endpoint_command(kind: u32, slot: u32, dci: u32) -> [u32; 4] {
    [0, 0, 0, (slot << 24) | (dci << 16) | (kind << 10)]
}

fn set_dequeue_command(slot: u32, dci: u32, pointer: u64) -> [u32; 4] {
    [
        (pointer | 1) as u32,
        ((pointer | 1) >> 32) as u32,
        0,
        (slot << 24) | (dci << 16) | (TRB_TYPE_SET_TR_DEQUEUE_POINTER << 10),
    ]
}

// ---- TRB builders (cycle bit 1)

fn setup_trb(
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
    trt: u32,
    flags: u32,
) -> [u32; 4] {
    [
        u32::from(request_type) | (u32::from(request) << 8) | (u32::from(value) << 16),
        u32::from(index) | (u32::from(length) << 16),
        8,
        (2 << 10) | IDT | (trt << 16) | flags | 1,
    ]
}

fn data_trb(buffer: u64, length: u32, dir_in: bool, flags: u32) -> [u32; 4] {
    [
        buffer as u32,
        (buffer >> 32) as u32,
        length,
        (3 << 10) | (u32::from(dir_in) << 16) | flags | 1,
    ]
}

fn normal_trb(buffer: u64, length: u32, flags: u32) -> [u32; 4] {
    [
        buffer as u32,
        (buffer >> 32) as u32,
        length,
        (1 << 10) | flags | 1,
    ]
}

fn status_trb(dir_in: bool, flags: u32) -> [u32; 4] {
    [0, 0, 0, (4 << 10) | (u32::from(dir_in) << 16) | flags | 1]
}

/// GET_DESCRIPTOR(device) of `length` bytes: setup (TRT IN).
fn get_descriptor(length: u16, flags: u32) -> [u32; 4] {
    setup_trb(0x80, 6, 0x0100, 0, length, 3, flags)
}

fn set_configuration(flags: u32) -> [u32; 4] {
    setup_trb(0x00, 9, 1, 0, 0, 0, flags)
}

struct Fixture {
    rig: Rig,
    device: TestDevice,
    slot: u32,
}

impl Rig {
    fn put_ring(&mut self, base: u64, trbs: &[[u32; 4]]) {
        for (index, trb) in trbs.iter().enumerate() {
            self.put_trb(base + 16 * index as u64, *trb);
        }
    }

    fn ring_endpoint(&mut self, slot: u32, dci: u32) {
        self.set_dword(doorbell(u64::from(slot)), dci);
    }

    /// Transfer Events and nothing else, in ring order; ERDP follows.
    fn transfer_events(&mut self) -> Vec<Event> {
        let events = self.events();
        assert!(
            events.iter().all(|e| e.kind == TRB_TYPE_TRANSFER_EVENT),
            "{events:?}"
        );
        events
    }

    fn out_ep_context(&mut self, slot: u32, dci: u32) -> [u32; 8] {
        let raw = self.mem_read(output(slot) + 32 * u64::from(dci), 32);
        let mut context = [0u32; 8];
        for (dword, chunk) in context.iter_mut().zip(raw.chunks_exact(4)) {
            *dword = u32::from_le_bytes(chunk.try_into().unwrap());
        }
        context
    }

    /// Stop Endpoint: both of its events are consumed.
    fn stop_endpoint(&mut self, slot: u32, dci: u32) {
        self.push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, slot, dci));
        self.ring_command_doorbell();
        self.service();
        assert_eq!(self.events().len(), 2);
    }

    fn command_ok(&mut self, trb: [u32; 4]) {
        assert_eq!(self.command(trb).code, CC_SUCCESS);
    }
}

fn write_input(rig: &mut Rig, add: u32, slot: [u32; 8], ep0: [u32; 8], ep3: [u32; 8]) {
    let mut put = |address: u64, context: [u32; 8]| {
        let mut raw = Vec::new();
        for dword in context {
            raw.extend_from_slice(&dword.to_le_bytes());
        }
        rig.mem_write(address, &raw);
    };
    put(INPUT, [0, add, 0, 0, 0, 0, 0, 0]);
    put(INPUT + 32, slot);
    put(INPUT + 64, ep0);
    put(INPUT + 128, ep3);
}

/// A running controller with a scripted device on `port`, the port reset, a slot Addressed
/// (BSR = 0) whose EP0 ring is at [`EP0_RING`].
fn addressed(port: u32) -> Fixture {
    addressed_with_ring(port, EP0_RING)
}

fn addressed_with_ring(port: u32, ep0_ring: u64) -> Fixture {
    let mut rig = running_rig();
    let device = TestDevice::default();
    rig.xhci
        .attach_device(port as usize, Box::new(device.clone()))
        .unwrap();
    rig.set_dword(portsc(u64::from(port)), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.set_dword(portsc(u64::from(port)), PORTSC_PRC);
    rig.service();
    rig.events();
    let slot = rig.enable_slot().slot_id;
    rig.mem_write(output(slot), &vec![0u8; 0x1000]);
    rig.mem_write(DCBAA + 8 * u64::from(slot), &output(slot).to_le_bytes());
    write_input(
        &mut rig,
        0b11,
        slot_ctx(port, 1),
        ep_ctx(4, 64, ep0_ring, 0),
        [0; 8],
    );
    rig.command_ok(pointer_command(TRB_TYPE_ADDRESS_DEVICE, INPUT, slot, 0));
    Fixture { rig, device, slot }
}

/// An Addressed slot with the interrupt endpoint configured (ring at [`INTERRUPT_RING`]).
fn configured(port: u32) -> Fixture {
    let mut f = addressed(port);
    let mps = if port == 1 { 8 } else { 6 };
    write_input(
        &mut f.rig,
        0b1001,
        slot_ctx(port, 3),
        [0; 8],
        ep_ctx(7, mps, INTERRUPT_RING, 6),
    );
    f.rig.command_ok(pointer_command(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        INPUT,
        f.slot,
        0,
    ));
    f
}

use crate::xhci::trb::{TRB_TYPE_ADDRESS_DEVICE, TRB_TYPE_CONFIGURE_ENDPOINT};

impl Fixture {
    fn state(&self, dci: u32) -> EndpointState {
        self.rig.xhci.endpoint_state(self.slot as usize, dci)
    }
    fn ring(&mut self, dci: u32) {
        self.rig.ring_endpoint(self.slot, dci);
        self.rig.service();
    }
}

// ---------------------------------------------------------------- control transfers

fn device_bytes() -> Vec<u8> {
    (0..18).map(|i| 0xA0 + i as u8).collect()
}

#[test]
fn control_in_transfer_returns_the_descriptor_and_completes_the_status_stage() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(device_bytes()));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16, CC_SUCCESS)
    );
    assert_eq!(events[0].raw[2] & 0xFF_FFFF, 0);
    assert_eq!(
        (events[0].slot_id, (events[0].raw[3] >> 16) & 0x1F),
        (f.slot, EP0)
    );
    assert_eq!(
        (events[1].parameter, events[1].code),
        (EP0_RING + 32, CC_SUCCESS)
    );
    assert_eq!(f.rig.mem_read(BUF, 18), device_bytes());
    let requests = f.device.requests();
    assert_eq!(requests, vec![([0x80, 6, 0, 1, 0, 0, 18, 0], vec![])]);
    assert_eq!(f.state(EP0), EndpointState::Running);
    let context = f.rig.out_ep_context(f.slot, EP0);
    assert_eq!(
        context[2],
        (EP0_RING + 48) as u32 | 1,
        "the dequeue pointer is behind the TD"
    );
}

#[test]
fn control_transfer_without_data_has_only_a_status_stage() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, IOC)]);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16, CC_SUCCESS)
    );
    assert_eq!(f.device.requests()[0].0[1], 9);
}

#[test]
fn control_setup_stage_with_ioc_completes_with_its_own_event() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(IOC), status_trb(true, 0)]);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].parameter, EP0_RING);
    assert_eq!(events[0].code, CC_SUCCESS);
}

#[test]
fn control_out_transfer_passes_the_data_to_the_device() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.rig.mem_write(BUF, &[0xDE, 0xAD, 0xBE]);
    f.rig.put_ring(
        EP0_RING,
        &[
            setup_trb(0x21, 9, 0x0200, 0, 3, 2, 0),
            data_trb(BUF, 3, false, IOC),
            status_trb(true, IOC),
        ],
    );
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events().len(), 2);
    assert_eq!(f.device.requests()[0].1, vec![0xDE, 0xAD, 0xBE]);
}

#[test]
fn control_out_transfer_takes_immediate_data_from_the_trb() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    let mut data = data_trb(0, 1, false, IOC | IDT);
    data[0] = 0x05;
    f.rig.put_ring(
        EP0_RING,
        &[
            setup_trb(0x21, 9, 0x0200, 0, 1, 2, 0),
            data,
            status_trb(true, IOC),
        ],
    );
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events().len(), 2);
    assert_eq!(f.device.requests()[0].1, vec![0x05]);
}

#[test]
fn control_data_stage_chain_crosses_the_page_boundary() {
    let mut f = addressed(1);
    let config: Vec<u8> = (0..34).map(|i| i as u8).collect();
    f.device.respond(ControlResponse::Data(config.clone()));
    // 20 bytes at the end of one page, 14 bytes in another buffer; only the last TRB has IOC.
    f.rig.put_ring(
        EP0_RING,
        &[
            setup_trb(0x80, 6, 0x0200, 0, 34, 3, 0),
            data_trb(BUF + 0xFEC, 20, true, CH),
            normal_trb(BUF2, 14, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 32, CC_SUCCESS)
    );
    assert_eq!(f.rig.mem_read(BUF + 0xFEC, 20), config[..20]);
    assert_eq!(f.rig.mem_read(BUF2, 14), config[20..]);
}

#[test]
fn control_data_stage_chain_reports_every_trb_with_ioc() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![7; 16]));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(16, 0),
            data_trb(BUF, 8, true, CH | IOC),
            normal_trb(BUF2, 8, IOC),
            status_trb(false, 0),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        events.iter().map(|e| e.parameter).collect::<Vec<_>>(),
        vec![EP0_RING + 16, EP0_RING + 32]
    );
}

#[test]
fn control_chain_may_pass_through_a_link_trb() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![9; 16]));
    let target = EP0_RING + 0x100;
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(16, 0),
            data_trb(BUF, 8, true, CH),
            [
                target as u32,
                (target >> 32) as u32,
                0,
                (TRB_TYPE_LINK << 10) | 2 | 1,
            ],
        ],
    );
    // After the Link TRB's Toggle Cycle the ring expects cycle 0.
    let mut normal = normal_trb(BUF2, 8, IOC);
    normal[3] &= !1;
    let mut status = status_trb(false, IOC);
    status[3] &= !1;
    // A stale TRB (cycle 1) ends the ring after the Status Stage.
    f.rig
        .put_ring(target, &[normal, status, status_trb(false, 0)]);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].parameter, target);
    let context = f.rig.out_ep_context(f.slot, EP0);
    assert_eq!(
        context[2],
        (target + 32) as u32,
        "dequeue behind the Status Stage, DCS 0"
    );
}

#[test]
fn control_short_packet_with_isp_reports_the_remainder() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![1; 8]));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, ISP),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16, CC_SHORT_PACKET)
    );
    assert_eq!(events[0].raw[2] & 0xFF_FFFF, 10);
    assert_eq!(
        events[1].code, CC_SUCCESS,
        "the Status Stage still completes"
    );
    assert_eq!(f.rig.mem_read(BUF, 8), vec![1; 8]);
}

#[test]
fn control_short_packet_with_ioc_only_is_also_a_short_packet_event() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![1; 8]));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SHORT_PACKET, 10)
    );
}

#[test]
fn control_short_packet_without_isp_or_ioc_gives_no_data_stage_event() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![1; 8]));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, 0),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1, "only the Status Stage event");
    assert_eq!(events[0].parameter, EP0_RING + 32);
    assert_eq!(
        f.rig.mem_read(BUF, 8),
        vec![1; 8],
        "the data was transferred"
    );
}

#[test]
fn control_chain_short_packet_events_follow_the_contract() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![5; 12]));
    // 8 + 8 + 8 bytes, the device returns 12: the short packet is in the second TRB (ISP only);
    // the first TRB (IOC) completed, the third (IOC) is skipped but reports the same remainder.
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(24, 0),
            data_trb(BUF, 8, true, CH | IOC),
            normal_trb(BUF + 0x100, 8, CH | ISP),
            normal_trb(BUF + 0x200, 8, IOC),
            status_trb(false, 0),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    let summary: Vec<_> = events
        .iter()
        .map(|e| (e.parameter, e.code, e.raw[2] & 0xFF_FFFF))
        .collect();
    assert_eq!(
        summary,
        vec![
            (EP0_RING + 16, CC_SUCCESS, 0),
            (EP0_RING + 32, CC_SHORT_PACKET, 4),
            (EP0_RING + 48, CC_SHORT_PACKET, 4),
        ]
    );
    assert_eq!(f.rig.mem_read(BUF + 0x100, 4), vec![5; 4]);
}

#[test]
fn control_transfer_length_is_limited_by_wlength() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(device_bytes()));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(8, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, 0),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SHORT_PACKET, 10)
    );
    assert_eq!(
        f.rig.mem_read(BUF + 8, 4),
        vec![0; 4],
        "nothing beyond wLength is written"
    );
}

#[test]
fn trt_dir_and_wlength_mismatches_are_not_checked_by_the_controller() {
    // xHCI 4.11.2.2: the controller does not check TRT, DIR or the sum of the lengths against wLength.
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(device_bytes()));
    f.rig.put_ring(
        EP0_RING,
        &[
            // TRT says "no data", the request says 4 bytes IN, the Data Stage has 18 and DIR OUT
            // for the Status Stage as well.
            setup_trb(0x80, 6, 0x0100, 0, 4, 0, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].code, CC_SHORT_PACKET,
        "4 bytes of wLength against 18 queued"
    );
    assert_eq!(events[1].code, CC_SUCCESS);
    assert_eq!(f.state(EP0), EndpointState::Running);
}

#[test]
fn a_status_stage_without_ioc_produces_no_event() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, 0)]);
    f.ring(EP0);
    assert!(f.rig.events().is_empty());
    assert_eq!(
        f.rig.out_ep_context(f.slot, EP0)[2],
        (EP0_RING + 32) as u32 | 1
    );
}

#[test]
fn block_event_interrupt_writes_the_event_without_an_interrupt() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![1; 16]));
    f.rig.set_dword(IMAN, IP | IE);
    f.rig.set_dword(USBSTS, EINT);
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(16, 0),
            data_trb(BUF, 8, true, CH),
            normal_trb(BUF2, 8, IOC | BEI),
            status_trb(false, 0),
        ],
    );
    f.ring(EP0);
    assert_eq!(f.rig.iman() & IP, 0);
    assert!(!f.rig.xhci.sample_pci_interrupts());
    assert_eq!(f.rig.transfer_events().len(), 1);
}

#[test]
fn block_event_interrupt_is_ignored_in_the_reserved_bit_of_a_status_stage() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.rig.set_dword(IMAN, IP | IE);
    f.rig.put_ring(
        EP0_RING,
        &[set_configuration(0), status_trb(true, IOC | BEI)],
    );
    f.ring(EP0);
    assert_eq!(f.rig.iman() & IP, IP);
}

#[test]
fn a_zero_length_trb_in_a_chain_takes_no_data_and_the_chain_goes_on() {
    let mut f = addressed(1);
    let bytes: Vec<u8> = (0..34).collect();
    f.device.respond(ControlResponse::Data(bytes.clone()));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(34, 0),
            data_trb(BUF, 8, true, CH),
            normal_trb(BUF + 0x100, 0, CH),
            normal_trb(BUF2, 26, IOC),
            status_trb(false, 0),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SUCCESS, 0)
    );
    assert_eq!(f.rig.mem_read(BUF2, 26), bytes[8..]);
}

#[test]
fn an_interrupt_in_chain_reports_the_remainder_of_the_whole_td() {
    let mut f = configured(1);
    f.device.report(&[1; 6]);
    f.rig.put_ring(
        INTERRUPT_RING,
        &[normal_trb(BUF, 8, CH), normal_trb(BUF2, 8, IOC)],
    );
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SUCCESS, 10)
    );
}

#[test]
fn an_interrupt_in_nak_needs_no_room_in_the_event_ring() {
    let mut f = configured(1);
    // 14 No Ops leave two free TRBs; polling an idle endpoint must not write the Full Error.
    for _ in 0..14 {
        f.rig.push_command(no_op());
    }
    f.rig.ring_command_doorbell();
    f.rig.service();
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.ring(EP3);
    f.rig.service();
    let events = f.rig.peek_events();
    assert_eq!(events.len(), 14, "only the completions");
    assert!(
        events
            .iter()
            .all(|e| e.kind == TRB_TYPE_COMMAND_COMPLETION_EVENT)
    );
}

#[test]
fn the_dequeue_pointer_skips_a_link_trb_behind_a_finished_td() {
    let mut f = configured(1);
    // The first TD is followed by a Link TRB back to the start of the segment (Toggle Cycle).
    f.rig.put_ring(
        INTERRUPT_RING,
        &[
            normal_trb(BUF, 8, IOC),
            [
                INTERRUPT_RING as u32 + 0x100,
                0,
                0,
                (TRB_TYPE_LINK << 10) | 2 | 1,
            ],
        ],
    );
    f.rig.put_trb(
        INTERRUPT_RING + 0x100,
        [BUF as u32 + 0x100, 0, 8, (1 << 10) | IOC],
    );
    f.device.report(&[1; 8]);
    f.ring(EP3);
    f.rig.events();
    // The next TRB (cycle 0 after the toggle) is already valid: Stop Endpoint sees it.
    let context = f.rig.out_ep_context(f.slot, EP3);
    assert_eq!(
        context[2],
        (INTERRUPT_RING + 0x100) as u32,
        "behind the Link TRB, DCS 0"
    );
    f.rig
        .push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, f.slot, EP3));
    f.rig.ring_command_doorbell();
    f.rig.service();
    let events = f.rig.events();
    assert_eq!(events[0].code, CC_STOPPED);
    assert_eq!(events[0].parameter, INTERRUPT_RING + 0x100);
}

#[test]
fn a_ring_of_link_trbs_that_cannot_be_passed_is_a_trb_error() {
    let mut f = addressed(1);
    f.rig
        .put_trb(EP0_RING, [EP0_RING as u32, 0, 0, (TRB_TYPE_LINK << 10) | 1]);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events[0].code, CC_TRB_ERROR);
    assert_eq!(f.state(EP0), EndpointState::Error);
    assert!(!f.rig.xhci.service_pending());
}

#[test]
fn an_incomplete_control_td_waits_for_its_status_stage() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.rig.put_ring(EP0_RING, &[set_configuration(0)]);
    f.ring(EP0);
    assert!(
        f.device.requests().is_empty(),
        "nothing is executed half-queued"
    );
    assert!(!f.rig.xhci.service_pending());
    f.rig.put_trb(EP0_RING + 16, status_trb(true, IOC));
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events().len(), 1);
}

#[test]
fn a_doorbell_on_an_empty_control_ring_does_nothing() {
    let mut f = addressed(1);
    f.ring(EP0);
    assert!(f.rig.events().is_empty());
    assert_eq!(f.state(EP0), EndpointState::Running);
}

#[test]
fn two_control_tds_in_one_doorbell_both_run() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Ack);
    f.device.respond(ControlResponse::Ack);
    f.rig.put_ring(
        EP0_RING,
        &[
            set_configuration(0),
            status_trb(true, IOC),
            set_configuration(0),
            status_trb(true, IOC),
        ],
    );
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events().len(), 2);
    assert_eq!(f.device.requests().len(), 2);
}

#[test]
fn a_default_slot_runs_control_transfers_with_the_device_at_address_zero() {
    let mut rig = running_rig();
    let device = TestDevice::default();
    rig.xhci.attach_device(1, Box::new(device.clone())).unwrap();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.set_dword(portsc(1), PORTSC_PRC);
    rig.service();
    rig.events();
    let slot = rig.enable_slot().slot_id;
    rig.mem_write(output(slot), &vec![0u8; 0x1000]);
    rig.mem_write(DCBAA + 8 * u64::from(slot), &output(slot).to_le_bytes());
    write_input(
        &mut rig,
        0b11,
        slot_ctx(1, 1),
        ep_ctx(4, 64, EP0_RING, 0),
        [0; 8],
    );
    rig.command_ok(pointer_command(
        TRB_TYPE_ADDRESS_DEVICE,
        INPUT,
        slot,
        1 << 9,
    ));
    device.respond(ControlResponse::Data(device_bytes()));
    rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(8, 0),
            data_trb(BUF, 8, true, IOC),
            status_trb(false, 0),
        ],
    );
    rig.ring_endpoint(slot, EP0);
    rig.service();
    assert_eq!(rig.transfer_events()[0].code, CC_SUCCESS);
}

#[test]
fn control_transfers_are_limited_by_the_trb_budget_of_one_service_call() {
    let mut f = addressed_with_ring(1, BIG_RING);
    // 130 no-data TDs (setup + status, no IOC) = 260 TRBs; one call consumes 256 TRBs.
    let mut trbs = Vec::new();
    for _ in 0..130 {
        trbs.push(set_configuration(0));
        trbs.push(status_trb(true, 0));
    }
    f.rig.put_ring(BIG_RING, &trbs);
    for _ in 0..130 {
        f.device.respond(ControlResponse::Ack);
    }
    f.ring(EP0);
    assert_eq!(f.device.requests().len(), 128);
    assert!(
        f.rig.xhci.service_pending(),
        "the rest waits for the next call"
    );
    f.rig.service();
    assert_eq!(f.device.requests().len(), 130);
    assert!(!f.rig.xhci.service_pending());
}

// ---- errors of control transfers

#[test]
fn a_stalled_request_halts_ep0_with_a_stall_error_on_the_data_stage() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Stall);
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1, "the Status Stage is not executed");
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16, CC_STALL_ERROR)
    );
    assert_eq!(events[0].raw[2] & 0xFF_FFFF, 18);
    assert_eq!(f.state(EP0), EndpointState::Halted);
    assert_eq!(
        f.rig.out_ep_context(f.slot, EP0)[0] & 7,
        2,
        "EP State Halted in the context"
    );
    // A doorbell on a Halted endpoint is ignored.
    f.rig.ring_endpoint(f.slot, EP0);
    assert_eq!(f.rig.xhci.ignored_doorbells(), 1);
}

#[test]
fn a_stalled_request_without_data_stalls_at_the_status_stage() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Stall);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, IOC)]);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16, CC_STALL_ERROR)
    );
    assert_eq!(f.state(EP0), EndpointState::Halted);
}

#[test]
fn ep0_recovers_from_a_stall_with_reset_endpoint_and_set_tr_dequeue_pointer() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Stall);
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    f.rig.events();
    // The driver skips the failed TD and queues the next one behind it.
    f.rig
        .command_ok(endpoint_command(TRB_TYPE_RESET_ENDPOINT, f.slot, EP0));
    assert_eq!(f.state(EP0), EndpointState::Stopped);
    f.rig
        .command_ok(set_dequeue_command(f.slot, EP0, EP0_RING + 0x100));
    f.device.respond(ControlResponse::Data(device_bytes()));
    f.rig.put_ring(
        EP0_RING + 0x100,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    assert_eq!(f.state(EP0), EndpointState::Running);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].parameter),
        (CC_SUCCESS, EP0_RING + 0x110)
    );
}

#[test]
fn set_address_in_a_setup_stage_is_a_trb_error_and_puts_ep0_in_error() {
    let mut f = addressed(1);
    f.rig.put_ring(
        EP0_RING,
        &[setup_trb(0x00, 5, 9, 0, 0, 0, IOC), status_trb(true, IOC)],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING, CC_TRB_ERROR)
    );
    assert_eq!(f.state(EP0), EndpointState::Error);
    assert!(
        f.device.requests().is_empty(),
        "the device never sees SET_ADDRESS"
    );
    assert_eq!(f.rig.out_ep_context(f.slot, EP0)[0] & 7, 4);
}

#[test]
fn ep0_recovers_from_a_trb_error_with_set_tr_dequeue_pointer() {
    let mut f = addressed(1);
    f.rig.put_ring(
        EP0_RING,
        &[setup_trb(0x00, 5, 9, 0, 0, 0, 0), status_trb(true, 0)],
    );
    f.ring(EP0);
    f.rig.events();
    f.rig
        .command_ok(set_dequeue_command(f.slot, EP0, EP0_RING + 0x100));
    assert_eq!(f.state(EP0), EndpointState::Stopped, "Error to Stopped");
    f.device.respond(ControlResponse::Ack);
    f.rig.put_ring(
        EP0_RING + 0x100,
        &[set_configuration(0), status_trb(true, IOC)],
    );
    f.ring(EP0);
    assert_eq!(f.state(EP0), EndpointState::Running);
    assert_eq!(f.rig.transfer_events().len(), 1);
}

/// Queues a damaged control TD and expects TRB Error at TRB \`offender\` of the ring.
fn expect_control_trb_error(trbs: &[[u32; 4]], offender: u64) {
    let mut f = addressed(1);
    f.rig.put_ring(EP0_RING, trbs);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16 * offender, CC_TRB_ERROR)
    );
    assert_eq!(f.state(EP0), EndpointState::Error);
    assert!(f.device.requests().is_empty());
}

#[test]
fn a_setup_stage_without_immediate_data_is_a_trb_error() {
    let mut setup = get_descriptor(18, 0);
    setup[3] &= !IDT;
    expect_control_trb_error(
        &[setup, data_trb(BUF, 18, true, 0), status_trb(false, 0)],
        0,
    );
}

#[test]
fn a_setup_stage_with_a_length_other_than_8_is_a_trb_error() {
    let mut setup = get_descriptor(18, 0);
    setup[2] = 9;
    expect_control_trb_error(&[setup, status_trb(false, 0)], 0);
}

#[test]
fn a_chained_setup_stage_is_a_trb_error() {
    expect_control_trb_error(
        &[
            get_descriptor(18, CH),
            data_trb(BUF, 18, true, 0),
            status_trb(false, 0),
        ],
        0,
    );
}

#[test]
fn a_setup_stage_with_an_interrupter_target_is_a_trb_error() {
    let mut setup = get_descriptor(18, 0);
    setup[2] |= 1 << 22;
    expect_control_trb_error(&[setup, status_trb(false, 0)], 0);
}

#[test]
fn a_ring_that_does_not_start_with_a_setup_stage_is_a_trb_error() {
    expect_control_trb_error(&[status_trb(false, 0)], 0);
    expect_control_trb_error(&[normal_trb(BUF, 8, 0)], 0);
}

#[test]
fn event_data_and_transfer_no_op_trbs_are_a_trb_error() {
    // Types 7 and 8 are v1 deviations (contract section 8).
    let trb = |kind: u32| [0, 0, 0, (kind << 10) | 1];
    expect_control_trb_error(&[trb(7)], 0);
    expect_control_trb_error(&[trb(8)], 0);
    expect_control_trb_error(&[get_descriptor(18, 0), trb(7)], 1);
    expect_control_trb_error(&[get_descriptor(18, 0), trb(8)], 1);
}

#[test]
fn a_chained_status_stage_is_a_trb_error() {
    expect_control_trb_error(&[set_configuration(0), status_trb(true, CH)], 1);
}

#[test]
fn a_status_stage_with_an_interrupter_target_is_a_trb_error() {
    let mut status = status_trb(true, 0);
    status[2] |= 1 << 22;
    expect_control_trb_error(&[set_configuration(0), status], 1);
}

#[test]
fn a_data_stage_chain_with_a_non_normal_trb_is_a_trb_error() {
    expect_control_trb_error(
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 8, true, CH),
            status_trb(false, 0),
        ],
        2,
    );
}

#[test]
fn a_data_stage_chain_of_more_than_64_trbs_is_a_trb_error() {
    let mut trbs = vec![get_descriptor(65, 0), data_trb(BUF, 1, true, CH)];
    for _ in 0..64 {
        trbs.push(normal_trb(BUF, 1, CH));
    }
    trbs.push(status_trb(false, 0));
    expect_control_trb_error(&trbs, 65);
}

#[test]
fn a_data_stage_chain_of_64_trbs_is_accepted() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(vec![3; 64]));
    let mut trbs = vec![get_descriptor(64, 0), data_trb(BUF, 1, true, CH)];
    for _ in 0..62 {
        trbs.push(normal_trb(BUF + 0x100, 1, CH));
    }
    trbs.push(normal_trb(BUF + 0x100, 1, IOC));
    trbs.push(status_trb(false, IOC));
    f.rig.put_ring(EP0_RING, &trbs);
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events().len(), 2);
}

#[test]
fn a_data_stage_longer_than_65535_bytes_is_a_trb_error() {
    expect_control_trb_error(
        &[
            get_descriptor(100, 0),
            data_trb(BUF, 40_000, true, CH),
            normal_trb(BUF, 30_000, 0),
            status_trb(false, 0),
        ],
        1,
    );
}

#[test]
fn immediate_data_in_an_in_data_stage_is_a_trb_error() {
    expect_control_trb_error(
        &[
            get_descriptor(4, 0),
            data_trb(0, 4, true, IDT),
            status_trb(false, 0),
        ],
        1,
    );
}

#[test]
fn immediate_data_of_more_than_8_bytes_is_a_trb_error() {
    expect_control_trb_error(
        &[
            setup_trb(0x21, 9, 0, 0, 9, 2, 0),
            data_trb(0, 9, false, IDT),
            status_trb(true, 0),
        ],
        1,
    );
}

#[test]
fn a_data_buffer_outside_ram_is_a_data_buffer_error_and_halts_ep0() {
    let mut f = addressed(1);
    f.device.respond(ControlResponse::Data(device_bytes()));
    f.rig.put_ring(
        EP0_RING,
        &[
            get_descriptor(18, 0),
            data_trb(UNMAPPED, 18, true, IOC),
            status_trb(false, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING + 16, CC_DATA_BUFFER_ERROR)
    );
    assert_eq!(f.state(EP0), EndpointState::Halted);
}

#[test]
fn an_out_buffer_outside_ram_is_a_data_buffer_error_and_the_device_is_not_asked() {
    let mut f = addressed(1);
    f.rig.put_ring(
        EP0_RING,
        &[
            setup_trb(0x21, 9, 0x0200, 0, 3, 2, 0),
            data_trb(UNMAPPED, 3, false, IOC),
            status_trb(true, IOC),
        ],
    );
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(events[0].code, CC_DATA_BUFFER_ERROR);
    assert_eq!(f.state(EP0), EndpointState::Halted);
    assert!(f.device.requests().is_empty());
}

#[test]
fn a_device_that_lost_its_address_gives_a_usb_transaction_error_and_halts_ep0() {
    let mut f = addressed(1);
    // A port reset puts the device back to address 0; the slot still says Addressed.
    f.rig.set_dword(portsc(1), PORTSC_PR);
    f.rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    f.rig.set_dword(portsc(1), PORTSC_PRC);
    f.rig.service();
    f.rig.events();
    f.device.respond(ControlResponse::Ack);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, IOC)]);
    f.ring(EP0);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (EP0_RING, CC_USB_TRANSACTION_ERROR)
    );
    assert_eq!(f.state(EP0), EndpointState::Halted);
    assert!(f.device.requests().is_empty());
}

#[test]
fn a_disabled_port_gives_a_usb_transaction_error() {
    let mut f = addressed(1);
    f.rig.set_dword(portsc(1), PORTSC_PED);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, IOC)]);
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events()[0].code, CC_USB_TRANSACTION_ERROR);
}

#[test]
fn a_control_td_waits_for_room_in_the_event_ring() {
    let mut f = addressed(1);
    // 14 No Ops leave two TRBs: a TD with an event needs room for it and the reserved TRB.
    for _ in 0..14 {
        f.rig.push_command(no_op());
    }
    f.rig.ring_command_doorbell();
    f.rig.service();
    f.device.respond(ControlResponse::Ack);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, IOC)]);
    f.rig.ring_endpoint(f.slot, EP0);
    f.rig.service();
    assert!(
        f.device.requests().is_empty(),
        "not executed without room for its events"
    );
    assert!(!f.rig.xhci.service_pending());
    let before = f.rig.peek_events().len();
    assert_eq!(before, 15, "14 completions and the Event Ring Full Error");
    f.rig.acknowledge_events();
    assert!(f.rig.xhci.service_pending());
    f.rig.service();
    assert_eq!(f.device.requests().len(), 1);
    assert_eq!(f.rig.events().last().unwrap().kind, TRB_TYPE_TRANSFER_EVENT);
}

#[test]
fn a_stopped_endpoint_does_not_run_until_its_doorbell() {
    let mut f = addressed(1);
    f.rig
        .push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, f.slot, EP0));
    f.rig.ring_command_doorbell();
    f.rig.service();
    f.rig.events();
    f.device.respond(ControlResponse::Ack);
    f.rig
        .put_ring(EP0_RING, &[set_configuration(0), status_trb(true, IOC)]);
    f.rig.service();
    assert!(f.device.requests().is_empty());
    f.ring(EP0);
    assert_eq!(f.rig.transfer_events().len(), 1);
}

// ---------------------------------------------------------------- Interrupt IN

#[test]
fn interrupt_in_delivers_a_keyboard_report() {
    let mut f = configured(1);
    f.rig.put_ring(
        INTERRUPT_RING,
        &[normal_trb(BUF, 8, IOC), normal_trb(BUF + 0x100, 8, IOC)],
    );
    f.ring(EP3);
    assert!(
        f.rig.events().is_empty(),
        "no report: the TRB is not consumed"
    );
    f.device.report(&[0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    f.rig.service();
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (INTERRUPT_RING, CC_SUCCESS)
    );
    assert_eq!(events[0].raw[2] & 0xFF_FFFF, 0);
    assert_eq!(
        (events[0].slot_id, (events[0].raw[3] >> 16) & 0x1F),
        (f.slot, EP3)
    );
    assert_eq!(f.rig.mem_read(BUF, 8), vec![0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(
        f.rig.out_ep_context(f.slot, EP3)[2],
        (INTERRUPT_RING + 16) as u32 | 1
    );
    assert_eq!(f.state(EP3), EndpointState::Running);
}

#[test]
fn interrupt_in_naks_until_a_report_is_ready_without_a_new_doorbell() {
    let mut f = configured(1);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.ring(EP3);
    assert!(
        f.rig.xhci.service_pending(),
        "the endpoint polls the device"
    );
    f.rig.service();
    assert!(f.rig.events().is_empty());
    f.device.report(&[1, 2, 3, 4, 5, 6, 7, 8]);
    f.rig.service();
    assert_eq!(f.rig.transfer_events().len(), 1);
    assert!(!f.rig.xhci.service_pending(), "the ring is empty again");
}

#[test]
fn interrupt_in_gives_one_report_per_trb() {
    let mut f = configured(1);
    f.device.report(&[1; 8]);
    f.device.report(&[2; 8]);
    f.rig.put_ring(
        INTERRUPT_RING,
        &[
            normal_trb(BUF, 8, IOC),
            normal_trb(BUF + 0x100, 8, IOC),
            normal_trb(BUF + 0x200, 8, IOC),
        ],
    );
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events().len(), 2);
    assert_eq!(f.rig.mem_read(BUF, 8), vec![1; 8]);
    assert_eq!(f.rig.mem_read(BUF + 0x100, 8), vec![2; 8]);
    assert_eq!(
        f.rig.mem_read(BUF + 0x200, 8),
        vec![0; 8],
        "the third TRB waits"
    );
}

#[test]
fn interrupt_in_short_report_with_isp_is_a_short_packet() {
    let mut f = configured(1);
    f.device.report(&[9; 6]);
    f.rig
        .put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC | ISP)]);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SHORT_PACKET, 2)
    );
}

#[test]
fn interrupt_in_short_report_without_isp_is_a_success_with_the_remainder() {
    let mut f = configured(1);
    f.device.report(&[9; 6]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SUCCESS, 2)
    );
}

#[test]
fn interrupt_in_report_of_the_pointer_fits_a_six_byte_trb() {
    let mut f = configured(2);
    f.device.report(&[1, 2, 3, 4, 5, 6]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 6, IOC)]);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].code, events[0].raw[2] & 0xFF_FFFF),
        (CC_SUCCESS, 0)
    );
}

#[test]
fn interrupt_in_report_fills_a_chain_in_order() {
    let mut f = configured(1);
    f.device.report(&[1, 2, 3, 4, 5, 6, 7, 8]);
    f.rig.put_ring(
        INTERRUPT_RING,
        &[normal_trb(BUF, 4, CH), normal_trb(BUF2, 4, IOC)],
    );
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].parameter, events[0].code),
        (INTERRUPT_RING + 16, CC_SUCCESS)
    );
    assert_eq!(f.rig.mem_read(BUF, 4), vec![1, 2, 3, 4]);
    assert_eq!(f.rig.mem_read(BUF2, 4), vec![5, 6, 7, 8]);
}

#[test]
fn interrupt_in_without_ioc_consumes_the_trb_silently() {
    let mut f = configured(1);
    f.device.report(&[7; 8]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, 0)]);
    f.ring(EP3);
    assert!(f.rig.events().is_empty());
    assert_eq!(f.rig.mem_read(BUF, 8), vec![7; 8]);
    assert_eq!(
        f.rig.out_ep_context(f.slot, EP3)[2],
        (INTERRUPT_RING + 16) as u32 | 1
    );
}

#[test]
fn interrupt_in_block_event_interrupt_writes_the_event_quietly() {
    let mut f = configured(1);
    f.rig.set_dword(IMAN, IP | IE);
    f.device.report(&[7; 8]);
    f.rig
        .put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC | BEI)]);
    f.ring(EP3);
    assert_eq!(f.rig.iman() & IP, 0);
    assert_eq!(f.rig.transfer_events().len(), 1);
}

#[test]
fn a_td_shorter_than_the_report_is_a_babble_error_and_halts_the_endpoint() {
    let mut f = configured(1);
    f.device.report(&[4; 8]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 4, IOC)]);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (INTERRUPT_RING, CC_BABBLE_DETECTED_ERROR)
    );
    assert_eq!(f.state(EP3), EndpointState::Halted);
    // The report stays in the device: after recovery a big enough TRB gets it.
    f.rig
        .command_ok(endpoint_command(TRB_TYPE_RESET_ENDPOINT, f.slot, EP3));
    f.rig
        .command_ok(set_dequeue_command(f.slot, EP3, INTERRUPT_RING + 0x100));
    f.rig
        .put_ring(INTERRUPT_RING + 0x100, &[normal_trb(BUF2, 8, IOC)]);
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events()[0].code, CC_SUCCESS);
    assert_eq!(f.rig.mem_read(BUF2, 8), vec![4; 8]);
}

#[test]
fn a_halted_device_endpoint_gives_a_stall_error_and_keeps_the_trb() {
    let mut f = configured(1);
    f.device.with(|s| s.halted = true);
    f.device.report(&[1; 8]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (INTERRUPT_RING, CC_STALL_ERROR)
    );
    assert_eq!(f.state(EP3), EndpointState::Halted);
    assert_eq!(
        f.rig.mem_read(BUF, 8),
        vec![0; 8],
        "the TRB was not consumed"
    );
    assert_eq!(
        f.rig.out_ep_context(f.slot, EP3)[2],
        INTERRUPT_RING as u32 | 1
    );
}

#[test]
fn interrupt_in_with_a_non_normal_trb_is_a_trb_error() {
    let mut f = configured(1);
    f.rig.put_ring(INTERRUPT_RING, &[status_trb(true, IOC)]);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (INTERRUPT_RING, CC_TRB_ERROR)
    );
    assert_eq!(f.state(EP3), EndpointState::Error);
}

#[test]
fn interrupt_in_with_an_interrupter_target_is_a_trb_error() {
    let mut f = configured(1);
    let mut trb = normal_trb(BUF, 8, IOC);
    trb[2] |= 1 << 22;
    f.rig.put_ring(INTERRUPT_RING, &[trb]);
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events()[0].code, CC_TRB_ERROR);
    assert_eq!(f.state(EP3), EndpointState::Error);
}

#[test]
fn interrupt_in_with_immediate_data_is_a_trb_error() {
    let mut f = configured(1);
    f.rig
        .put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC | IDT)]);
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events()[0].code, CC_TRB_ERROR);
}

#[test]
fn interrupt_in_chain_of_more_than_64_trbs_is_a_trb_error() {
    let mut f = configured(1);
    let mut trbs: Vec<[u32; 4]> = (0..65).map(|_| normal_trb(BUF, 1, CH)).collect();
    trbs.push(normal_trb(BUF, 1, IOC));
    f.rig.put_ring(INTERRUPT_RING, &trbs);
    f.ring(EP3);
    let events = f.rig.transfer_events();
    assert_eq!(
        (events[0].parameter, events[0].code),
        (INTERRUPT_RING + 64 * 16, CC_TRB_ERROR)
    );
}

#[test]
fn an_interrupt_buffer_outside_ram_is_a_data_buffer_error() {
    let mut f = configured(1);
    f.device.report(&[1; 8]);
    f.rig
        .put_ring(INTERRUPT_RING, &[normal_trb(UNMAPPED, 8, IOC)]);
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events()[0].code, CC_DATA_BUFFER_ERROR);
    assert_eq!(f.state(EP3), EndpointState::Halted);
}

#[test]
fn interrupt_in_after_a_port_reset_is_a_usb_transaction_error() {
    let mut f = configured(1);
    f.rig.set_dword(portsc(1), PORTSC_PR);
    f.rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    f.rig.set_dword(portsc(1), PORTSC_PRC);
    f.rig.service();
    f.rig.events();
    f.device.report(&[1; 8]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events()[0].code, CC_USB_TRANSACTION_ERROR);
    assert_eq!(f.state(EP3), EndpointState::Halted);
}

#[test]
fn a_stopped_interrupt_endpoint_gets_no_report_until_its_doorbell() {
    let mut f = configured(1);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.rig
        .push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, f.slot, EP3));
    f.rig.ring_command_doorbell();
    f.rig.service();
    let events = f.rig.events();
    assert_eq!(events[0].code, CC_STOPPED, "the queued TRB is still valid");
    f.device.report(&[3; 8]);
    f.rig.service();
    assert!(f.rig.events().is_empty());
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events().len(), 1);
    assert_eq!(f.rig.mem_read(BUF, 8), vec![3; 8]);
}

#[test]
fn interrupt_in_transfers_are_limited_by_the_trb_budget() {
    let mut f = configured(1);
    let trbs: Vec<[u32; 4]> = (0..300).map(|_| normal_trb(BUF, 8, 0)).collect();
    // The interrupt ring region holds 256 TRBs; use a bigger ring through Set TR Dequeue Pointer.
    f.rig.stop_endpoint(f.slot, EP3);
    f.rig.command_ok(set_dequeue_command(f.slot, EP3, BIG_RING));
    f.rig.put_ring(BIG_RING, &trbs);
    for _ in 0..300 {
        f.device.report(&[1; 8]);
    }
    f.ring(EP3);
    assert_eq!(f.device.with(|s| s.reports.len()), 300 - 256);
    assert!(f.rig.xhci.service_pending());
    f.rig.service();
    assert_eq!(f.device.with(|s| s.reports.len()), 0);
}

#[test]
fn hcrst_and_port_resets_reset_the_device() {
    let mut f = addressed(1);
    let after_setup = f.device.with(|s| s.resets);
    f.rig.set_dword(USBCMD, HCRST);
    assert_eq!(f.device.with(|s| s.resets), after_setup + 1);
    assert_eq!(f.device.with(|s| s.address), 0);
}

#[test]
fn a_report_longer_than_announced_is_a_babble_error() {
    let mut f = configured(1);
    f.device.with(|s| s.announce = Some(4));
    f.device.report(&[1; 8]);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 4, IOC)]);
    f.ring(EP3);
    assert_eq!(f.rig.transfer_events()[0].code, CC_BABBLE_DETECTED_ERROR);
    assert_eq!(f.state(EP3), EndpointState::Halted);
}
