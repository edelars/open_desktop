//! Tests of the slot and endpoint commands (S8-42, slice B): Address Device, Configure Endpoint,
//! Evaluate Context, Stop Endpoint, Reset Endpoint, Set TR Dequeue Pointer and Reset Device, with
//! the Input and Output Device Contexts in guest RAM. Every refusal of the contract has its test.

use super::*;
use crate::xhci::slots::EndpointState;
use crate::xhci::trb::{
    CC_CONTEXT_STATE_ERROR, CC_PARAMETER_ERROR, CC_STOPPED, CC_STOPPED_LENGTH_INVALID,
    CC_USB_TRANSACTION_ERROR, TRB_TYPE_ADDRESS_DEVICE, TRB_TYPE_CONFIGURE_ENDPOINT,
    TRB_TYPE_EVALUATE_CONTEXT, TRB_TYPE_LINK, TRB_TYPE_RESET_DEVICE, TRB_TYPE_RESET_ENDPOINT,
    TRB_TYPE_SET_TR_DEQUEUE_POINTER, TRB_TYPE_STOP_ENDPOINT, TRB_TYPE_TRANSFER_EVENT,
};

type Ctx = [u32; 8];

const INPUT: u64 = 0x5_0000;
const OUTPUT_BASE: u64 = 0x6_0000;
const EP0_RING: u64 = 0x7_0000;
const INTERRUPT_RING: u64 = 0x7_1000;
const BSR_OR_DC: u32 = 1 << 9;

const EP0: u32 = 1;
const EP3: u32 = 3;

fn output(slot: u32) -> u64 {
    OUTPUT_BASE + u64::from(slot) * 0x1000
}

fn slot_ctx(port: u32, entries: u32) -> Ctx {
    [entries << 27, port << 16, 0, 0, 0, 0, 0, 0]
}

fn ep_ctx(kind: u32, mps: u32, dequeue: u64, dcs: bool, interval: u32) -> Ctx {
    let raw = dequeue | u64::from(dcs);
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

fn ep0_ctx() -> Ctx {
    ep_ctx(4, 64, EP0_RING, true, 0)
}

fn interrupt_ctx() -> Ctx {
    ep_ctx(7, 8, INTERRUPT_RING, true, 6)
}

/// An Input Context under construction.
#[derive(Clone, Copy)]
struct Input {
    drop: u32,
    add: u32,
    slot: Ctx,
    ep0: Ctx,
    ep3: Ctx,
}

impl Input {
    /// Address Device: A0 and A1, a Slot Context for the port, a control EP0.
    fn address(port: u32) -> Self {
        Self {
            drop: 0,
            add: 0b11,
            slot: slot_ctx(port, 1),
            ep0: ep0_ctx(),
            ep3: [0; 8],
        }
    }

    /// Configure Endpoint: A0 and A3 with the keyboard's interrupt endpoint.
    fn configure() -> Self {
        Self {
            drop: 0,
            add: 0b1001,
            slot: slot_ctx(1, 3),
            ep0: [0; 8],
            ep3: interrupt_ctx(),
        }
    }
}

fn command_with_pointer(kind: u32, pointer: u64, slot_id: u32, flags: u32) -> [u32; 4] {
    [
        pointer as u32,
        (pointer >> 32) as u32,
        0,
        (slot_id << 24) | (kind << 10) | flags,
    ]
}

fn endpoint_command(kind: u32, slot_id: u32, dci: u32) -> [u32; 4] {
    [0, 0, 0, (slot_id << 24) | (dci << 16) | (kind << 10)]
}

impl Rig {
    fn write_ctx(&mut self, address: u64, context: &Ctx) {
        let mut raw = Vec::new();
        for dword in context {
            raw.extend_from_slice(&dword.to_le_bytes());
        }
        self.mem_write(address, &raw);
    }

    fn read_ctx(&mut self, address: u64) -> Ctx {
        let raw = self.mem_read(address, 32);
        let mut context = [0u32; 8];
        for (dword, chunk) in context.iter_mut().zip(raw.chunks_exact(4)) {
            *dword = u32::from_le_bytes(chunk.try_into().unwrap());
        }
        context
    }

    fn write_input(&mut self, input: &Input) {
        self.write_ctx(INPUT, &[input.drop, input.add, 0, 0, 0, 0, 0, 0]);
        self.write_ctx(INPUT + 32, &input.slot);
        self.write_ctx(INPUT + 64, &input.ep0);
        self.write_ctx(INPUT + 128, &input.ep3);
    }

    fn out_slot(&mut self, slot: u32) -> Ctx {
        self.read_ctx(output(slot))
    }

    fn out_ep(&mut self, slot: u32, dci: u32) -> Ctx {
        self.read_ctx(output(slot) + 32 * u64::from(dci))
    }

    /// DCBAA[slot] points at a zeroed Output Device Context.
    fn set_output_context(&mut self, slot: u32) {
        self.mem_write(output(slot), &vec![0u8; 0x1000]);
        self.mem_write(DCBAA + 8 * u64::from(slot), &output(slot).to_le_bytes());
    }

    /// Resets a port and consumes what the reset reports.
    fn reset_port(&mut self, port: u64) {
        self.set_dword(portsc(port), PORTSC_PR);
        self.advance_ns(XHCI_USB_PORT_RESET_NS);
        self.set_dword(portsc(port), PORTSC_PRC);
        self.service();
        self.events();
        self.set_dword(IMAN, IP | IE);
    }

    fn address_command(&mut self, slot: u32, input: &Input, bsr: bool) -> Event {
        self.write_input(input);
        self.command(command_with_pointer(
            TRB_TYPE_ADDRESS_DEVICE,
            INPUT,
            slot,
            if bsr { BSR_OR_DC } else { 0 },
        ))
    }

    /// A slot Enabled with an Output Device Context, port 1 reset.
    fn enabled_slot(&mut self) -> u32 {
        let slot = self.enable_slot().slot_id;
        self.set_output_context(slot);
        slot
    }

    /// Enables a slot on a reset port and addresses it (BSR = 0).
    fn addressed_slot(&mut self, port: u32) -> u32 {
        self.reset_port(u64::from(port));
        let slot = self.enabled_slot();
        let event = self.address_command(slot, &Input::address(port), false);
        assert_eq!(event.code, CC_SUCCESS);
        slot
    }

    fn configure_command(&mut self, slot: u32, input: &Input) -> Event {
        self.write_input(input);
        self.command(command_with_pointer(
            TRB_TYPE_CONFIGURE_ENDPOINT,
            INPUT,
            slot,
            0,
        ))
    }

    /// An Addressed slot with the interrupt endpoint added.
    fn configured_slot(&mut self, port: u32) -> u32 {
        let slot = self.addressed_slot(port);
        let event = self.configure_command(slot, &Input::configure());
        assert_eq!(event.code, CC_SUCCESS);
        slot
    }

    fn evaluate_command(&mut self, slot: u32, input: &Input) -> Event {
        self.write_input(input);
        self.command(command_with_pointer(
            TRB_TYPE_EVALUATE_CONTEXT,
            INPUT,
            slot,
            0,
        ))
    }

    fn stop(&mut self, slot: u32, dci: u32) -> Vec<Event> {
        self.push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, slot, dci));
        self.ring_command_doorbell();
        self.service();
        self.events()
    }

    fn slot_state_field(&mut self, slot: u32) -> u32 {
        self.out_slot(slot)[3] >> 27
    }
}

// ---------------------------------------------------------------- Address Device

#[test]
fn address_device_with_bsr_0_sets_the_address_and_enters_addressed() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!((event.code, event.slot_id), (CC_SUCCESS, slot));
    let out = rig.out_slot(slot);
    assert_eq!(out[3] & 0xFF, slot, "USB device address = Slot ID");
    assert_eq!(out[3] >> 27, 2, "Slot State Addressed");
    assert_eq!(out[0] >> 27, 1, "Context Entries");
    assert_eq!((out[1] >> 16) & 0xFF, 1, "Root Hub Port Number copied");
    assert_eq!(
        rig.xhci.device_address(1),
        slot as u8,
        "SET_ADDRESS reached the device"
    );
    let ep0 = rig.out_ep(slot, EP0);
    assert_eq!(ep0[0] & 7, 1, "EP0 Running");
    assert_eq!(ep0[1] >> 16, 64, "Max Packet Size copied");
    assert_eq!(
        ep0[2],
        EP0_RING as u32 | 1,
        "TR Dequeue Pointer and DCS copied"
    );
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Running
    );
}

#[test]
fn address_device_with_bsr_1_enters_default_with_address_0() {
    let mut rig = running_rig();
    rig.reset_port(2);
    let slot = rig.enabled_slot();
    let event = rig.address_command(slot, &Input::address(2), true);
    assert_eq!(event.code, CC_SUCCESS);
    let out = rig.out_slot(slot);
    assert_eq!(out[3] & 0xFF, 0);
    assert_eq!(out[3] >> 27, 1, "Slot State Default");
    assert_eq!(rig.xhci.device_address(2), 0, "no SET_ADDRESS is sent");
    assert_eq!(rig.out_ep(slot, EP0)[0] & 7, 1);
}

#[test]
fn address_device_from_default_with_bsr_0_completes_the_addressing() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.address_command(slot, &Input::address(1), true);
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!(event.code, CC_SUCCESS);
    assert_eq!(rig.slot_state_field(slot), 2);
    assert_eq!(rig.out_slot(slot)[3] & 0xFF, slot);
}

#[test]
fn address_device_input_context_pointer_must_be_64_byte_aligned() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.write_input(&Input::address(1));
    let event = rig.command(command_with_pointer(
        TRB_TYPE_ADDRESS_DEVICE,
        INPUT + 0x10,
        slot,
        0,
    ));
    assert_eq!(event.code, CC_TRB_ERROR);
    assert_eq!(rig.slot_state_field(slot), 0, "nothing was written");
}

#[test]
fn address_device_for_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    rig.reset_port(1);
    rig.write_input(&Input::address(1));
    let event = rig.command(command_with_pointer(TRB_TYPE_ADDRESS_DEVICE, INPUT, 5, 0));
    assert_eq!((event.code, event.slot_id), (CC_SLOT_NOT_ENABLED, 5));
}

#[test]
fn address_device_for_an_addressed_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
    let event = rig.address_command(slot, &Input::address(1), true);
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn address_device_with_bsr_1_for_a_default_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.address_command(slot, &Input::address(1), true);
    let event = rig.address_command(slot, &Input::address(1), true);
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn address_device_for_a_configured_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

/// Runs Address Device with a damaged Input Context on a fresh slot of a reset port.
fn address_parameter_error(damage: impl FnOnce(&mut Input)) {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    let mut input = Input::address(1);
    damage(&mut input);
    let event = rig.address_command(slot, &input, false);
    assert_eq!(event.code, CC_PARAMETER_ERROR);
    assert_eq!(
        rig.slot_state_field(slot),
        0,
        "the Output Device Context is untouched"
    );
    assert_eq!(rig.xhci.device_address(1), 0);
    // The slot is still Enabled: a valid retry works.
    assert_eq!(
        rig.address_command(slot, &Input::address(1), false).code,
        CC_SUCCESS
    );
}

#[test]
fn address_device_with_a_drop_flag_is_a_parameter_error() {
    address_parameter_error(|input| input.drop = 1 << 2);
}

#[test]
fn address_device_without_a0_is_a_parameter_error() {
    address_parameter_error(|input| input.add = 0b10);
}

#[test]
fn address_device_without_a1_is_a_parameter_error() {
    address_parameter_error(|input| input.add = 0b01);
}

#[test]
fn address_device_with_another_add_flag_is_a_parameter_error() {
    address_parameter_error(|input| input.add = 0b111);
}

#[test]
fn address_device_with_a_route_string_is_a_parameter_error() {
    address_parameter_error(|input| input.slot[0] |= 0x1);
}

#[test]
fn address_device_with_root_port_0_is_a_parameter_error() {
    address_parameter_error(|input| input.slot = slot_ctx(0, 1));
}

#[test]
fn address_device_with_root_port_3_is_a_parameter_error() {
    address_parameter_error(|input| input.slot = slot_ctx(3, 1));
}

#[test]
fn address_device_with_an_interrupter_target_is_a_parameter_error() {
    address_parameter_error(|input| input.slot[2] |= 1 << 22);
}

#[test]
fn address_device_with_context_entries_other_than_1_is_a_parameter_error() {
    address_parameter_error(|input| input.slot = slot_ctx(1, 2));
    address_parameter_error(|input| input.slot = slot_ctx(1, 0));
}

#[test]
fn address_device_with_a_non_control_ep0_is_a_parameter_error() {
    address_parameter_error(|input| input.ep0 = ep_ctx(7, 64, EP0_RING, true, 0));
}

#[test]
fn address_device_with_ep0_max_packet_size_below_8_is_a_parameter_error() {
    address_parameter_error(|input| input.ep0 = ep_ctx(4, 7, EP0_RING, true, 0));
}

#[test]
fn address_device_with_a_zero_ep0_dequeue_pointer_is_a_parameter_error() {
    address_parameter_error(|input| input.ep0 = ep_ctx(4, 64, 0, true, 0));
}

#[test]
fn address_device_with_reserved_bits_in_the_ep0_dequeue_pointer_is_a_parameter_error() {
    address_parameter_error(|input| input.ep0[2] |= 0x4);
}

#[test]
fn address_device_with_ep0_dcs_0_is_a_parameter_error() {
    address_parameter_error(|input| input.ep0 = ep_ctx(4, 64, EP0_RING, false, 0));
}

#[test]
fn address_device_with_ep0_streams_is_a_parameter_error() {
    address_parameter_error(|input| input.ep0[0] |= 1 << 10);
}

#[test]
fn address_device_for_a_port_that_is_not_enabled_is_a_usb_transaction_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!(
        event.code, CC_USB_TRANSACTION_ERROR,
        "the port was not reset"
    );
    assert_eq!(rig.slot_state_field(slot), 0);
    assert_eq!(rig.xhci.device_address(1), 0);
    // The state did not change: after the reset the same slot can be addressed.
    rig.reset_port(1);
    assert_eq!(
        rig.address_command(slot, &Input::address(1), false).code,
        CC_SUCCESS
    );
}

#[test]
fn address_device_after_a_port_was_disabled_is_a_usb_transaction_error() {
    let mut rig = running_rig();
    rig.reset_port(2);
    rig.set_dword(portsc(2), PORTSC_PED);
    let slot = rig.enabled_slot();
    let event = rig.address_command(slot, &Input::address(2), false);
    assert_eq!(event.code, CC_USB_TRANSACTION_ERROR);
}

#[test]
fn address_device_with_a_zero_dcbaa_entry_is_a_host_system_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enable_slot().slot_id;
    rig.write_input(&Input::address(1));
    rig.push_command(command_with_pointer(
        TRB_TYPE_ADDRESS_DEVICE,
        INPUT,
        slot,
        0,
    ));
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::ContextDma);
}

#[test]
fn address_device_with_a_dcbaa_entry_outside_ram_is_a_host_system_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enable_slot().slot_id;
    rig.mem_write(DCBAA + 8 * u64::from(slot), &UNMAPPED.to_le_bytes());
    rig.write_input(&Input::address(1));
    rig.push_command(command_with_pointer(
        TRB_TYPE_ADDRESS_DEVICE,
        INPUT,
        slot,
        0,
    ));
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::ContextDma);
}

#[test]
fn address_device_with_an_unaligned_dcbaa_entry_is_a_host_system_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enable_slot().slot_id;
    rig.mem_write(
        DCBAA + 8 * u64::from(slot),
        &(output(slot) + 0x20).to_le_bytes(),
    );
    rig.write_input(&Input::address(1));
    rig.push_command(command_with_pointer(
        TRB_TYPE_ADDRESS_DEVICE,
        INPUT,
        slot,
        0,
    ));
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::ContextDma);
}

#[test]
fn address_device_with_an_input_context_outside_ram_is_a_host_system_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.push_command(command_with_pointer(
        TRB_TYPE_ADDRESS_DEVICE,
        UNMAPPED,
        slot,
        0,
    ));
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::ContextDma);
}

#[test]
fn a_port_reset_returns_the_device_to_the_default_state_but_not_the_slot() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.reset_port(1);
    assert_eq!(rig.xhci.device_address(1), 0);
    assert_eq!(rig.slot_state_field(slot), 2, "the slot keeps its state");
    // Address Device from Addressed is a Context State Error; Reset Device makes it possible again.
    assert_eq!(
        rig.address_command(slot, &Input::address(1), false).code,
        CC_CONTEXT_STATE_ERROR
    );
    assert_eq!(
        rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, slot, 0))
            .code,
        CC_SUCCESS
    );
    assert_eq!(
        rig.address_command(slot, &Input::address(1), false).code,
        CC_SUCCESS
    );
    assert_eq!(rig.xhci.device_address(1), slot as u8);
}

// ---------------------------------------------------------------- Configure Endpoint

#[test]
fn configure_endpoint_adds_the_interrupt_endpoint_and_configures_the_slot() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let event = rig.configure_command(slot, &Input::configure());
    assert_eq!((event.code, event.slot_id), (CC_SUCCESS, slot));
    let out = rig.out_slot(slot);
    assert_eq!(out[3] >> 27, 3, "Slot State Configured");
    assert_eq!(out[0] >> 27, 3, "Context Entries covers DCI 3");
    let ep3 = rig.out_ep(slot, EP3);
    assert_eq!(ep3[0] & 7, 1, "EP State Running");
    assert_eq!((ep3[0] >> 16) & 0xFF, 6, "Interval 6 (10 ms at Full-speed)");
    assert_eq!(ep3[2], INTERRUPT_RING as u32 | 1);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Running
    );
}

#[test]
fn configure_endpoint_accepts_the_pointer_report_length_on_port_2() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(2);
    let mut input = Input::configure();
    input.ep3 = ep_ctx(7, 6, INTERRUPT_RING, true, 6);
    assert_eq!(rig.configure_command(slot, &input).code, CC_SUCCESS);
}

#[test]
fn configure_endpoint_accepts_intervals_3_to_10() {
    for interval in [3, 10] {
        let mut rig = running_rig();
        let slot = rig.addressed_slot(1);
        let mut input = Input::configure();
        input.ep3 = ep_ctx(7, 8, INTERRUPT_RING, true, interval);
        assert_eq!(
            rig.configure_command(slot, &input).code,
            CC_SUCCESS,
            "interval {interval}"
        );
    }
}

#[test]
fn configure_endpoint_for_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let event = rig.configure_command(4, &Input::configure());
    assert_eq!((event.code, event.slot_id), (CC_SLOT_NOT_ENABLED, 4));
}

#[test]
fn configure_endpoint_for_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    assert_eq!(
        rig.configure_command(slot, &Input::configure()).code,
        CC_CONTEXT_STATE_ERROR
    );
}

#[test]
fn configure_endpoint_for_a_default_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.address_command(slot, &Input::address(1), true);
    assert_eq!(
        rig.configure_command(slot, &Input::configure()).code,
        CC_CONTEXT_STATE_ERROR
    );
}

#[test]
fn configure_endpoint_input_context_pointer_must_be_64_byte_aligned() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.write_input(&Input::configure());
    let event = rig.command(command_with_pointer(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        INPUT + 0x10,
        slot,
        0,
    ));
    assert_eq!(event.code, CC_TRB_ERROR);
}

#[test]
fn configure_endpoint_drop_and_add_in_one_command_replaces_the_endpoint() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    let mut input = Input::configure();
    input.drop = 1 << 3;
    input.ep3 = ep_ctx(7, 8, INTERRUPT_RING + 0x100, true, 7);
    let event = rig.configure_command(slot, &input);
    assert_eq!(event.code, CC_SUCCESS);
    let ep3 = rig.out_ep(slot, EP3);
    assert_eq!(ep3[0] & 7, 1, "Running again");
    assert_eq!(
        (ep3[0] >> 16) & 0xFF,
        7,
        "reinitialized from the input context"
    );
    assert_eq!(ep3[2], (INTERRUPT_RING + 0x100) as u32 | 1);
    assert_eq!(rig.slot_state_field(slot), 3);
}

#[test]
fn configure_endpoint_drop_and_add_of_a_stopped_endpoint_restores_running() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    rig.stop(slot, EP3);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Stopped
    );
    let mut input = Input::configure();
    input.drop = 1 << 3;
    assert_eq!(rig.configure_command(slot, &input).code, CC_SUCCESS);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Running
    );
}

#[test]
fn configure_endpoint_add_of_a_running_endpoint_without_drop_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    assert_eq!(
        rig.configure_command(slot, &Input::configure()).code,
        CC_PARAMETER_ERROR
    );
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Running
    );
}

#[test]
fn configure_endpoint_drop_alone_disables_the_endpoint_and_returns_to_addressed() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    let mut input = Input::configure();
    input.add = 1;
    input.drop = 1 << 3;
    assert_eq!(rig.configure_command(slot, &input).code, CC_SUCCESS);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
    assert_eq!(rig.slot_state_field(slot), 2, "Addressed");
    assert_eq!(rig.out_slot(slot)[0] >> 27, 1);
    assert_eq!(rig.out_ep(slot, EP3), [0; 8]);
}

#[test]
fn configure_endpoint_drop_of_a_disabled_endpoint_is_ignored() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::configure();
    input.add = 1;
    input.drop = (1 << 3) | (1 << 5);
    assert_eq!(rig.configure_command(slot, &input).code, CC_SUCCESS);
    assert_eq!(rig.slot_state_field(slot), 2);
}

#[test]
fn configure_endpoint_without_a0_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::configure();
    input.add = 1 << 3;
    assert_eq!(rig.configure_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn configure_endpoint_with_d0_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::configure();
    input.drop = 1;
    assert_eq!(rig.configure_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn configure_endpoint_with_d1_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::configure();
    input.drop = 1 << 1;
    assert_eq!(rig.configure_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn configure_endpoint_with_a1_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::configure();
    input.add |= 1 << 1;
    assert_eq!(rig.configure_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn configure_endpoint_cannot_add_any_dci_but_3() {
    for dci in [2, 4, 5, 31] {
        let mut rig = running_rig();
        let slot = rig.addressed_slot(1);
        let mut input = Input::configure();
        input.add |= 1 << dci;
        assert_eq!(
            rig.configure_command(slot, &input).code,
            CC_PARAMETER_ERROR,
            "DCI {dci}"
        );
    }
}

/// Configure Endpoint with a damaged interrupt endpoint context on an Addressed slot of port 1.
fn configure_parameter_error(damage: impl FnOnce(&mut Ctx)) {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::configure();
    damage(&mut input.ep3);
    assert_eq!(rig.configure_command(slot, &input).code, CC_PARAMETER_ERROR);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
    assert_eq!(rig.slot_state_field(slot), 2);
    assert_eq!(
        rig.configure_command(slot, &Input::configure()).code,
        CC_SUCCESS
    );
}

#[test]
fn configure_endpoint_with_a_nonzero_input_ep_state_is_a_parameter_error() {
    configure_parameter_error(|ep| ep[0] |= 1);
}

#[test]
fn configure_endpoint_with_a_non_interrupt_in_type_is_a_parameter_error() {
    configure_parameter_error(|ep| *ep = ep_ctx(3, 8, INTERRUPT_RING, true, 6));
    configure_parameter_error(|ep| *ep = ep_ctx(4, 8, INTERRUPT_RING, true, 6));
}

#[test]
fn configure_endpoint_with_a_max_packet_size_below_the_report_is_a_parameter_error() {
    configure_parameter_error(|ep| *ep = ep_ctx(7, 7, INTERRUPT_RING, true, 6));
}

#[test]
fn configure_endpoint_on_port_2_with_a_max_packet_size_below_6_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(2);
    let mut input = Input::configure();
    input.ep3 = ep_ctx(7, 5, INTERRUPT_RING, true, 6);
    assert_eq!(rig.configure_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn configure_endpoint_with_an_interval_below_3_is_a_parameter_error() {
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, INTERRUPT_RING, true, 2));
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, INTERRUPT_RING, true, 0));
}

#[test]
fn configure_endpoint_with_an_interval_above_10_is_a_parameter_error() {
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, INTERRUPT_RING, true, 11));
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, INTERRUPT_RING, true, 15));
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, INTERRUPT_RING, true, 16));
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, INTERRUPT_RING, true, 255));
}

#[test]
fn configure_endpoint_with_a_zero_dequeue_pointer_is_a_parameter_error() {
    configure_parameter_error(|ep| *ep = ep_ctx(7, 8, 0, true, 6));
}

#[test]
fn configure_endpoint_with_reserved_bits_in_the_dequeue_pointer_is_a_parameter_error() {
    configure_parameter_error(|ep| ep[2] |= 0x2);
}

#[test]
fn configure_endpoint_with_max_burst_is_a_parameter_error() {
    configure_parameter_error(|ep| ep[1] |= 1 << 8);
}

#[test]
fn configure_endpoint_with_mult_is_a_parameter_error() {
    configure_parameter_error(|ep| ep[0] |= 1 << 8);
}

#[test]
fn configure_endpoint_with_streams_is_a_parameter_error() {
    configure_parameter_error(|ep| ep[0] |= 1 << 10);
}

#[test]
fn deconfigure_returns_a_configured_slot_to_addressed_and_disables_dci_3() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    let event = rig.command(command_with_pointer(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        0,
        slot,
        BSR_OR_DC,
    ));
    assert_eq!(
        event.code, CC_SUCCESS,
        "the Input Context Pointer is ignored"
    );
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
    assert_eq!(rig.slot_state_field(slot), 2);
    assert_eq!(rig.out_slot(slot)[0] >> 27, 1);
    assert_eq!(rig.out_ep(slot, EP3), [0; 8]);
}

#[test]
fn deconfigure_of_an_addressed_slot_without_dci_3_succeeds_without_action() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let before = rig.out_slot(slot);
    let event = rig.command(command_with_pointer(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        0,
        slot,
        BSR_OR_DC,
    ));
    assert_eq!(event.code, CC_SUCCESS);
    assert_eq!(rig.out_slot(slot), before);
}

#[test]
fn deconfigure_of_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let event = rig.command(command_with_pointer(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        0,
        slot,
        BSR_OR_DC,
    ));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn deconfigure_of_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let event = rig.command(command_with_pointer(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        0,
        6,
        BSR_OR_DC,
    ));
    assert_eq!(event.code, CC_SLOT_NOT_ENABLED);
}

#[test]
fn configure_endpoint_with_an_unreadable_input_context_is_a_host_system_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.push_command(command_with_pointer(
        TRB_TYPE_CONFIGURE_ENDPOINT,
        UNMAPPED,
        slot,
        0,
    ));
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::ContextDma);
}

// ---------------------------------------------------------------- Evaluate Context

#[test]
fn evaluate_context_updates_max_exit_latency_and_the_ep0_max_packet_size() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::address(1);
    input.add = 0b11;
    input.slot[1] |= 0x1234;
    input.ep0 = ep_ctx(4, 8, EP0_RING, true, 0);
    let event = rig.evaluate_command(slot, &input);
    assert_eq!((event.code, event.slot_id), (CC_SUCCESS, slot));
    assert_eq!(rig.out_slot(slot)[1] & 0xFFFF, 0x1234);
    assert_eq!(rig.out_ep(slot, EP0)[1] >> 16, 8);
    assert_eq!(
        rig.slot_state_field(slot),
        2,
        "the slot state is not touched"
    );
}

#[test]
fn evaluate_context_with_only_a1_updates_only_the_max_packet_size() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::address(1);
    input.add = 0b10;
    input.slot[1] |= 0x7777;
    input.ep0 = ep_ctx(4, 32, EP0_RING, true, 0);
    assert_eq!(rig.evaluate_command(slot, &input).code, CC_SUCCESS);
    assert_eq!(rig.out_slot(slot)[1] & 0xFFFF, 0);
    assert_eq!(rig.out_ep(slot, EP0)[1] >> 16, 32);
}

#[test]
fn evaluate_context_is_allowed_in_default_and_configured_slots() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.address_command(slot, &Input::address(1), true);
    let mut input = Input::address(1);
    input.add = 0b10;
    assert_eq!(
        rig.evaluate_command(slot, &input).code,
        CC_SUCCESS,
        "Default"
    );
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    assert_eq!(
        rig.evaluate_command(slot, &input).code,
        CC_SUCCESS,
        "Configured"
    );
}

#[test]
fn evaluate_context_for_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let mut input = Input::address(1);
    input.add = 0b10;
    let event = rig.evaluate_command(2, &input);
    assert_eq!((event.code, event.slot_id), (CC_SLOT_NOT_ENABLED, 2));
}

#[test]
fn evaluate_context_for_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let mut input = Input::address(1);
    input.add = 0b10;
    assert_eq!(
        rig.evaluate_command(slot, &input).code,
        CC_CONTEXT_STATE_ERROR
    );
}

#[test]
fn evaluate_context_with_a_drop_flag_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::address(1);
    input.drop = 1 << 3;
    assert_eq!(rig.evaluate_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn evaluate_context_ignores_add_flags_of_dci_2_and_above_within_context_entries() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::address(1);
    input.slot = slot_ctx(1, 5);
    input.add = 0b11 | (1 << 3) | (1 << 5);
    input.slot[1] |= 0x55;
    assert_eq!(rig.evaluate_command(slot, &input).code, CC_SUCCESS);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
    assert_eq!(
        rig.out_ep(slot, EP3),
        [0; 8],
        "the endpoint context is not evaluated"
    );
}

#[test]
fn evaluate_context_with_an_add_flag_above_the_context_entries_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::address(1);
    input.add = 0b11 | (1 << 4);
    assert_eq!(rig.evaluate_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn evaluate_context_with_an_interrupter_target_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let mut input = Input::address(1);
    input.add = 0b01;
    input.slot[2] |= 1 << 22;
    assert_eq!(rig.evaluate_command(slot, &input).code, CC_PARAMETER_ERROR);
}

#[test]
fn evaluate_context_input_context_pointer_must_be_64_byte_aligned() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.write_input(&Input::address(1));
    let event = rig.command(command_with_pointer(
        TRB_TYPE_EVALUATE_CONTEXT,
        INPUT + 0x20,
        slot,
        0,
    ));
    assert_eq!(event.code, CC_TRB_ERROR);
}

// ---------------------------------------------------------------- Stop Endpoint

fn put_interrupt_trb(rig: &mut Rig, address: u64, length: u32, cycle: bool) {
    rig.put_trb(address, [0x9_0000, 0, length, (1 << 10) | u32::from(cycle)]);
}

#[test]
fn stop_endpoint_writes_a_stopped_transfer_event_before_the_completion() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    put_interrupt_trb(&mut rig, INTERRUPT_RING, 8, true);
    let events = rig.stop(slot, EP3);
    assert_eq!(events.len(), 2);
    let transfer = events[0];
    assert_eq!(transfer.kind, TRB_TYPE_TRANSFER_EVENT);
    assert_eq!(transfer.code, CC_STOPPED);
    assert_eq!(
        transfer.parameter, INTERRUPT_RING,
        "the TRB at the dequeue pointer"
    );
    assert_eq!(
        transfer.raw[2] & 0xFF_FFFF,
        8,
        "its whole length is outstanding"
    );
    assert_eq!(transfer.slot_id, slot);
    assert_eq!((transfer.raw[3] >> 16) & 0x1F, EP3);
    let completion = events[1];
    assert_eq!(completion.kind, TRB_TYPE_COMMAND_COMPLETION_EVENT);
    assert_eq!((completion.code, completion.slot_id), (CC_SUCCESS, slot));
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Stopped
    );
}

#[test]
fn stop_endpoint_of_an_empty_ring_reports_stopped_length_invalid() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    let events = rig.stop(slot, EP3);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, TRB_TYPE_TRANSFER_EVENT);
    assert_eq!(events[0].code, CC_STOPPED_LENGTH_INVALID);
    assert_eq!(
        events[0].parameter, INTERRUPT_RING,
        "the TR Dequeue Pointer"
    );
    assert_eq!(events[0].raw[2] & 0xFF_FFFF, 0);
    assert_eq!(events[1].code, CC_SUCCESS);
}

#[test]
fn stop_endpoint_with_a_stale_trb_at_the_dequeue_pointer_reports_length_invalid() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    put_interrupt_trb(&mut rig, INTERRUPT_RING, 8, false);
    let events = rig.stop(slot, EP3);
    assert_eq!(events[0].code, CC_STOPPED_LENGTH_INVALID);
}

#[test]
fn stop_endpoint_stops_ep0_and_records_the_dequeue_pointer_and_dcs() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let events = rig.stop(slot, EP0);
    assert_eq!(events.len(), 2);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Stopped
    );
    let ep0 = rig.out_ep(slot, EP0);
    assert_eq!(
        ep0[0] & 7,
        3,
        "EP State Stopped in the Output Endpoint Context"
    );
    assert_eq!(ep0[2], EP0_RING as u32 | 1);
}

#[test]
fn stop_endpoint_of_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let events = rig.stop(3, EP0);
    assert_eq!(events.len(), 1, "no Transfer Event for a refused command");
    assert_eq!(
        (events[0].code, events[0].slot_id),
        (CC_SLOT_NOT_ENABLED, 3)
    );
}

#[test]
fn stop_endpoint_of_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let events = rig.stop(slot, EP0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn stop_endpoint_of_a_stopped_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    let events = rig.stop(slot, EP0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn stop_endpoint_of_a_disabled_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let events = rig.stop(slot, EP3);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].code, CC_CONTEXT_STATE_ERROR,
        "not Endpoint Not Enabled"
    );
}

#[test]
fn stop_endpoint_with_a_dci_the_model_does_not_have_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    for dci in [0, 2, 4, 31] {
        let events = rig.stop(slot, dci);
        assert_eq!(events.len(), 1, "DCI {dci}");
        assert_eq!(events[0].code, CC_CONTEXT_STATE_ERROR, "DCI {dci}");
    }
}

#[test]
fn stop_endpoint_of_a_halted_endpoint_is_a_context_state_error_without_a_transfer_event() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Halted);
    let events = rig.stop(slot, EP0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn stop_endpoint_of_an_endpoint_in_error_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Error);
    let events = rig.stop(slot, EP0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn stop_endpoint_waits_for_room_for_both_events() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    // 14 No Ops leave two free TRBs in the 16-TRB event ring: not enough for two events and
    // the reserved TRB.
    for _ in 0..14 {
        rig.push_command(no_op());
    }
    rig.push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, slot, EP0));
    rig.ring_command_doorbell();
    rig.service();
    let events = rig.peek_events();
    assert_eq!(events.len(), 15);
    assert_eq!(
        events[14].kind, TRB_TYPE_HOST_CONTROLLER_EVENT,
        "Event Ring Full Error"
    );
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Running
    );
    rig.acknowledge_events();
    rig.service();
    let events = rig.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, TRB_TYPE_TRANSFER_EVENT);
    assert_eq!(events[1].kind, TRB_TYPE_COMMAND_COMPLETION_EVENT);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Stopped
    );
}

// ---------------------------------------------------------------- Reset Endpoint

#[test]
fn reset_endpoint_turns_a_halted_endpoint_into_a_stopped_one() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Halted);
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, EP0));
    assert_eq!((event.code, event.slot_id), (CC_SUCCESS, slot));
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Stopped
    );
    assert_eq!(rig.out_ep(slot, EP0)[0] & 7, 3);
    assert_eq!(
        rig.out_ep(slot, EP0)[2],
        EP0_RING as u32 | 1,
        "the dequeue pointer is kept"
    );
    assert!(rig.peek_events().is_empty(), "no Transfer Event");
}

#[test]
fn reset_endpoint_of_a_running_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, EP0));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn reset_endpoint_of_a_stopped_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, EP0));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn reset_endpoint_of_an_endpoint_in_error_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Error);
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, EP0));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn reset_endpoint_of_a_disabled_endpoint_or_a_missing_dci_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    for dci in [0, 2, 3, 31] {
        let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, dci));
        assert_eq!(
            event.code, CC_CONTEXT_STATE_ERROR,
            "DCI {dci}: not Endpoint Not Enabled"
        );
    }
}

#[test]
fn reset_endpoint_of_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, 7, EP0));
    assert_eq!((event.code, event.slot_id), (CC_SLOT_NOT_ENABLED, 7));
}

#[test]
fn reset_endpoint_of_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, EP0));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

// ---------------------------------------------------------------- Set TR Dequeue Pointer

fn set_dequeue_command(slot: u32, dci: u32, pointer: u64, dcs: bool) -> [u32; 4] {
    let raw = pointer | u64::from(dcs);
    [
        raw as u32,
        (raw >> 32) as u32,
        0,
        (slot << 24) | (dci << 16) | (TRB_TYPE_SET_TR_DEQUEUE_POINTER << 10),
    ]
}

#[test]
fn set_tr_dequeue_pointer_moves_a_stopped_endpoint_ring() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    let new = EP0_RING + 0x40;
    let event = rig.command(set_dequeue_command(slot, EP0, new, false));
    assert_eq!((event.code, event.slot_id), (CC_SUCCESS, slot));
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Stopped
    );
    let ep0 = rig.out_ep(slot, EP0);
    assert_eq!(ep0[2], new as u32, "new pointer, DCS 0");
    assert_eq!(ep0[0] & 7, 3);
}

#[test]
fn set_tr_dequeue_pointer_after_a_reset_endpoint_recovers_a_halted_endpoint() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Halted);
    assert_eq!(
        rig.command(endpoint_command(TRB_TYPE_RESET_ENDPOINT, slot, EP0))
            .code,
        CC_SUCCESS
    );
    assert_eq!(
        rig.command(set_dequeue_command(slot, EP0, EP0_RING + 0x80, true))
            .code,
        CC_SUCCESS
    );
    // The doorbell restarts the endpoint.
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Running
    );
}

#[test]
fn set_tr_dequeue_pointer_moves_an_endpoint_in_error_to_stopped() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Error);
    let event = rig.command(set_dequeue_command(slot, EP0, EP0_RING + 0x20, true));
    assert_eq!(event.code, CC_SUCCESS);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Stopped
    );
}

#[test]
fn set_tr_dequeue_pointer_of_a_running_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let event = rig.command(set_dequeue_command(slot, EP0, EP0_RING + 0x20, true));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn set_tr_dequeue_pointer_of_a_halted_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Halted);
    let event = rig.command(set_dequeue_command(slot, EP0, EP0_RING + 0x20, true));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn set_tr_dequeue_pointer_of_a_disabled_endpoint_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    let event = rig.command(set_dequeue_command(slot, EP3, INTERRUPT_RING, true));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn set_tr_dequeue_pointer_with_a_stream_id_is_a_trb_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    let mut trb = set_dequeue_command(slot, EP0, EP0_RING + 0x20, true);
    trb[2] = 1 << 16;
    assert_eq!(rig.command(trb).code, CC_TRB_ERROR);
}

#[test]
fn set_tr_dequeue_pointer_with_a_stream_context_type_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    let mut trb = set_dequeue_command(slot, EP0, EP0_RING + 0x20, true);
    trb[0] |= 1 << 1;
    assert_eq!(rig.command(trb).code, CC_PARAMETER_ERROR);
}

#[test]
fn set_tr_dequeue_pointer_with_a_zero_pointer_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    assert_eq!(
        rig.command(set_dequeue_command(slot, EP0, 0, true)).code,
        CC_PARAMETER_ERROR
    );
}

#[test]
fn set_tr_dequeue_pointer_outside_ram_is_a_parameter_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    assert_eq!(
        rig.command(set_dequeue_command(slot, EP0, UNMAPPED, true))
            .code,
        CC_PARAMETER_ERROR
    );
    assert_eq!(
        rig.out_ep(slot, EP0)[2],
        EP0_RING as u32 | 1,
        "the context is unchanged"
    );
}

#[test]
fn set_tr_dequeue_pointer_of_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let event = rig.command(set_dequeue_command(8, EP0, EP0_RING, true));
    assert_eq!((event.code, event.slot_id), (CC_SLOT_NOT_ENABLED, 8));
}

#[test]
fn set_tr_dequeue_pointer_of_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let event = rig.command(set_dequeue_command(slot, EP0, EP0_RING, true));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

// ---------------------------------------------------------------- Reset Device

#[test]
fn reset_device_returns_the_slot_to_default_and_keeps_ep0() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, slot, 0));
    assert_eq!((event.code, event.slot_id), (CC_SUCCESS, slot));
    let out = rig.out_slot(slot);
    assert_eq!(out[3] & 0xFF, 0, "Device Address 0");
    assert_eq!(out[3] >> 27, 1, "Default");
    assert_eq!(out[0] >> 27, 1, "Context Entries 1");
    assert_eq!((out[1] >> 16) & 0xFF, 1, "Root Hub Port Number is kept");
    let ep0 = rig.out_ep(slot, EP0);
    assert_eq!(ep0[0] & 7, 1, "EP0 Running again");
    assert_eq!(ep0[1] >> 16, 64, "Max Packet Size kept");
    assert_eq!((ep0[1] >> 3) & 7, 4, "EP Type kept");
    assert_eq!(ep0[2], EP0_RING as u32 | 1, "dequeue pointer kept");
    assert_eq!(rig.out_ep(slot, EP3), [0; 8]);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
}

#[test]
fn reset_device_turns_a_stopped_or_halted_ep0_into_a_running_one() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Halted);
    assert_eq!(
        rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, slot, 0))
            .code,
        CC_SUCCESS
    );
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Running
    );
}

#[test]
fn address_device_after_reset_device_addresses_the_slot_again() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, slot, 0));
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!(event.code, CC_SUCCESS);
    assert_eq!(rig.slot_state_field(slot), 2);
}

#[test]
fn reset_device_of_a_slot_that_is_not_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, 4, 0));
    assert_eq!((event.code, event.slot_id), (CC_SLOT_NOT_ENABLED, 4));
}

#[test]
fn reset_device_of_an_enabled_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    let slot = rig.enabled_slot();
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, slot, 0));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

#[test]
fn reset_device_of_a_default_slot_is_a_context_state_error() {
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.address_command(slot, &Input::address(1), true);
    let event = rig.command(endpoint_command(TRB_TYPE_RESET_DEVICE, slot, 0));
    assert_eq!(event.code, CC_CONTEXT_STATE_ERROR);
}

// ---------------------------------------------------------------- Disable Slot, HCRST, doorbells

#[test]
fn disable_slot_disables_every_endpoint_and_the_next_enable_starts_clean() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    assert_eq!(rig.command(disable_slot(slot)).code, CC_SUCCESS);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Disabled
    );
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
    let again = rig.enable_slot().slot_id;
    assert_eq!(again, slot);
    assert_eq!(
        rig.configure_command(again, &Input::configure()).code,
        CC_CONTEXT_STATE_ERROR
    );
}

#[test]
fn hcrst_disables_all_slots_and_endpoints() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    rig.set_dword(USBCMD, HCRST);
    rig.start();
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Disabled
    );
    let event = rig.address_command(slot, &Input::address(1), false);
    assert_eq!(event.code, CC_SLOT_NOT_ENABLED);
}

#[test]
fn doorbell_restarts_a_stopped_endpoint() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    rig.stop(slot, EP3);
    rig.set_dword(doorbell(u64::from(slot)), EP3);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP3),
        EndpointState::Running
    );
    assert_eq!(rig.xhci.ignored_doorbells(), 0);
    assert!(rig.xhci.service_pending(), "the EP State mirror is owed");
    rig.service();
    assert!(
        !rig.xhci.service_pending(),
        "no transfer work is left behind"
    );
}

#[test]
fn doorbell_on_a_running_endpoint_is_accepted_and_changes_nothing() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    assert_eq!(rig.xhci.ignored_doorbells(), 0);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Running
    );
}

#[test]
fn doorbell_on_halted_error_and_disabled_endpoints_is_ignored() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Halted);
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Halted
    );
    rig.xhci
        .force_endpoint_state(slot as usize, EP0, EndpointState::Error);
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    assert_eq!(
        rig.xhci.endpoint_state(slot as usize, EP0),
        EndpointState::Error
    );
    rig.set_dword(doorbell(u64::from(slot)), EP3);
    assert_eq!(rig.xhci.ignored_doorbells(), 3);
}

#[test]
fn doorbell_on_dci_3_is_accepted_only_in_a_configured_slot() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.set_dword(doorbell(u64::from(slot)), EP3);
    assert_eq!(rig.xhci.ignored_doorbells(), 1, "Addressed slot");
    rig.configure_command(slot, &Input::configure());
    rig.set_dword(doorbell(u64::from(slot)), EP3);
    assert_eq!(rig.xhci.ignored_doorbells(), 1, "Configured slot");
}

#[test]
fn doorbell_of_ep0_in_a_default_slot_is_accepted_and_dci_3_is_ignored() {
    // The driver reads the device descriptor through EP0 of a Default slot (BSR = 1).
    let mut rig = running_rig();
    rig.reset_port(1);
    let slot = rig.enabled_slot();
    rig.address_command(slot, &Input::address(1), true);
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    assert_eq!(rig.xhci.ignored_doorbells(), 0);
    rig.set_dword(doorbell(u64::from(slot)), EP3);
    assert_eq!(rig.xhci.ignored_doorbells(), 1);
}

#[test]
fn doorbell_restart_is_mirrored_into_the_output_endpoint_context_by_service() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    assert_eq!(rig.out_ep(slot, EP0)[0] & 7, 3);
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    assert!(rig.xhci.service_pending());
    rig.service();
    assert_eq!(
        rig.out_ep(slot, EP0)[0] & 7,
        1,
        "Running again in guest memory"
    );
    assert_eq!(rig.out_ep(slot, EP0)[2], EP0_RING as u32 | 1);
    assert!(!rig.xhci.service_pending());
}

#[test]
fn doorbell_restart_with_a_broken_dcbaa_entry_is_a_host_system_error() {
    let mut rig = running_rig();
    let slot = rig.addressed_slot(1);
    rig.stop(slot, EP0);
    rig.set_dword(doorbell(u64::from(slot)), EP0);
    rig.mem_write(DCBAA + 8 * u64::from(slot), &0u64.to_le_bytes());
    rig.service();
    assert_host_system_error(&mut rig, HseCause::ContextDma);
}

#[test]
fn address_device_with_bsr_0_for_a_device_that_already_has_an_address_is_a_usb_transaction_error() {
    let mut rig = running_rig();
    rig.addressed_slot(1);
    let second = rig.enabled_slot();
    let event = rig.address_command(second, &Input::address(1), false);
    assert_eq!(
        event.code, CC_USB_TRANSACTION_ERROR,
        "the device answers to its address, not 0"
    );
    assert_eq!(rig.slot_state_field(second), 0);
    // BSR = 1 does not talk to the device; after a port reset SET_ADDRESS works again.
    assert_eq!(
        rig.address_command(second, &Input::address(1), true).code,
        CC_SUCCESS
    );
    let third = rig.enabled_slot();
    rig.reset_port(1);
    assert_eq!(
        rig.address_command(third, &Input::address(1), false).code,
        CC_SUCCESS
    );
}

#[test]
fn stop_endpoint_with_a_link_trb_at_the_dequeue_pointer_reports_length_invalid() {
    let mut rig = running_rig();
    let slot = rig.configured_slot(1);
    rig.put_trb(
        INTERRUPT_RING,
        [
            INTERRUPT_RING as u32 + 0x100,
            0,
            0,
            (TRB_TYPE_LINK << 10) | 1,
        ],
    );
    let events = rig.stop(slot, EP3);
    assert_eq!(events[0].code, CC_STOPPED_LENGTH_INVALID);
}
