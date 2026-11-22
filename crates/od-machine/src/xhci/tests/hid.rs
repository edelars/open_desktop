//! Tests of the HID devices (S8-43): descriptors, standard and class requests, input to report,
//! the report queue, and the enumeration of both devices through the controller. Every request
//! and every refusal of contract section 10 has its test.

use super::transfers::{
    BUF, BUF2, CH, EP0, EP0_RING, EP3, INTERRUPT_RING, IOC, TestDevice, add_interrupt_endpoint,
    addressed_with, data_trb, endpoint_command, get_descriptor, normal_trb, set_configuration,
    set_dequeue_command, setup_trb, status_trb,
};
use super::*;
use crate::xhci::hid::{HidDevice, HidHandle};
use crate::xhci::hid_descriptors::{
    HidKind, ReportDescriptor, configuration_descriptor, device_descriptor, hid_descriptor,
    report_descriptor, string_descriptor,
};
use crate::xhci::slots::EndpointState;
use crate::xhci::trb::{
    CC_STALL_ERROR, TRB_TYPE_RESET_ENDPOINT, TRB_TYPE_STOP_ENDPOINT, TRB_TYPE_TRANSFER_EVENT,
};
use crate::{ControlResponse, HidDevices, InputEvent, InputQueueFull, InterruptStatus, UsbDevice};

const KEYBOARD: HidKind = HidKind::Keyboard;
const POINTER: HidKind = HidKind::Pointer;

// ---------------------------------------------------------------- descriptors

#[rustfmt::skip]
const KEYBOARD_REPORT_DESCRIPTOR: [u8; 64] = [
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01,
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x26, 0xE7, 0x00, 0x05, 0x07, 0x19, 0x00, 0x29, 0xE7, 0x81, 0x00, 0xC0,
];

#[rustfmt::skip]
const POINTER_REPORT_DESCRIPTOR: [u8; 63] = [
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03,
    0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x03, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81,
    0x02, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0,
];

#[test]
fn the_keyboard_report_descriptor_is_encoded_from_its_items_to_64_bytes() {
    assert_eq!(report_descriptor(KEYBOARD), KEYBOARD_REPORT_DESCRIPTOR);
}

#[test]
fn the_pointer_report_descriptor_is_encoded_from_its_items_to_63_bytes() {
    assert_eq!(report_descriptor(POINTER), POINTER_REPORT_DESCRIPTOR);
}

#[test]
fn the_item_encoder_follows_hid_1_11_short_item_rules() {
    // Logical Maximum 0xE7 is signed: one byte would read as -25, so it takes two (26 E7 00).
    assert_eq!(
        ReportDescriptor::new().logical_maximum(0xE7).build(),
        [0x26, 0xE7, 0x00]
    );
    // Usage Maximum is unsigned: 29 E7.
    assert_eq!(
        ReportDescriptor::new().usage_maximum(0xE7).build(),
        [0x29, 0xE7]
    );
    assert_eq!(
        ReportDescriptor::new().logical_minimum(-127).build(),
        [0x15, 0x81]
    );
    assert_eq!(
        ReportDescriptor::new().logical_maximum(127).build(),
        [0x25, 0x7F]
    );
    assert_eq!(
        ReportDescriptor::new().logical_maximum(32767).build(),
        [0x26, 0xFF, 0x7F]
    );
    assert_eq!(
        ReportDescriptor::new().logical_minimum(0).build(),
        [0x15, 0x00]
    );
    assert_eq!(ReportDescriptor::new().end_collection().build(), [0xC0]);
    assert_eq!(
        ReportDescriptor::new().usage_page(1).usage(6).build(),
        [0x05, 0x01, 0x09, 0x06]
    );
    assert_eq!(
        ReportDescriptor::new().report_size(16).build(),
        [0x75, 0x10]
    );
    assert_eq!(ReportDescriptor::new().input(0x06).build(), [0x81, 0x06]);
    assert_eq!(ReportDescriptor::new().output(0x02).build(), [0x91, 0x02]);
    assert_eq!(ReportDescriptor::new().collection(1).build(), [0xA1, 0x01]);
}

#[test]
fn device_descriptors_follow_the_contract() {
    #[rustfmt::skip]
    let keyboard = [
        18, 1, 0x00, 0x02, 0, 0, 0, 64, 0xA6, 0xD1, 0x01, 0x01, 0x00, 0x01, 1, 2, 0, 1,
    ];
    assert_eq!(device_descriptor(KEYBOARD), keyboard);
    let mut pointer = keyboard;
    pointer[10] = 0x02;
    assert_eq!(device_descriptor(POINTER), pointer);
}

#[test]
fn configuration_descriptors_are_34_bytes_with_the_interface_hid_and_endpoint() {
    #[rustfmt::skip]
    let keyboard = [
        9, 2, 34, 0, 1, 1, 0, 0x80, 50,
        9, 4, 0, 0, 1, 3, 1, 1, 0,
        9, 0x21, 0x11, 0x01, 0, 1, 0x22, 64, 0,
        7, 5, 0x81, 0x03, 8, 0, 10,
    ];
    assert_eq!(configuration_descriptor(KEYBOARD), keyboard);
    #[rustfmt::skip]
    let pointer = [
        9, 2, 34, 0, 1, 1, 0, 0x80, 50,
        9, 4, 0, 0, 1, 3, 0, 0, 0,
        9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0,
        7, 5, 0x81, 0x03, 6, 0, 10,
    ];
    assert_eq!(configuration_descriptor(POINTER), pointer);
}

#[test]
fn the_hid_descriptor_length_is_the_length_of_the_report_descriptor() {
    for kind in [KEYBOARD, POINTER] {
        let hid = hid_descriptor(kind);
        assert_eq!(hid.len(), 9);
        assert_eq!(
            usize::from(u16::from_le_bytes([hid[7], hid[8]])),
            report_descriptor(kind).len()
        );
    }
}

#[test]
fn string_descriptors_are_utf16le() {
    let utf16 = |text: &str| -> Vec<u8> {
        let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut bytes = vec![(2 + units.len()) as u8, 3];
        bytes.extend(units);
        bytes
    };
    assert_eq!(string_descriptor(KEYBOARD, 0).unwrap(), [4, 3, 0x09, 0x04]);
    assert_eq!(
        string_descriptor(KEYBOARD, 1).unwrap(),
        utf16("Open Desktop")
    );
    assert_eq!(
        string_descriptor(POINTER, 1).unwrap(),
        utf16("Open Desktop")
    );
    assert_eq!(
        string_descriptor(KEYBOARD, 2).unwrap(),
        utf16("Virtual HID Keyboard")
    );
    assert_eq!(
        string_descriptor(POINTER, 2).unwrap(),
        utf16("Virtual HID Absolute Pointer")
    );
    assert!(string_descriptor(KEYBOARD, 3).is_none());
    assert!(string_descriptor(KEYBOARD, 0xEE).is_none());
}

// ---------------------------------------------------------------- control requests

fn request(
    device: &mut HidDevice,
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
    out_data: &[u8],
) -> ControlResponse {
    let mut setup = [0u8; 8];
    setup[0] = request_type;
    setup[1] = request;
    setup[2..4].copy_from_slice(&value.to_le_bytes());
    setup[4..6].copy_from_slice(&index.to_le_bytes());
    setup[6..8].copy_from_slice(&length.to_le_bytes());
    device.control(&setup, out_data)
}

fn data(bytes: &[u8]) -> ControlResponse {
    ControlResponse::Data(bytes.to_vec())
}

const STALL: ControlResponse = ControlResponse::Stall;
const ACK: ControlResponse = ControlResponse::Ack;

/// A device in the Address state (address 1).
fn addressed_device(kind: HidKind) -> (HidDevice, HidHandle) {
    let (mut device, handle) = HidDevice::new(kind);
    device.set_address(1);
    (device, handle)
}

/// A device that is configured, with a running interrupt endpoint.
fn configured_device(kind: HidKind) -> (HidDevice, HidHandle) {
    let (mut device, handle) = addressed_device(kind);
    assert_eq!(request(&mut device, 0x00, 9, 1, 0, 0, &[]), ACK);
    device.set_interrupt_endpoint_active(true);
    (device, handle)
}

fn get_status_device(d: &mut HidDevice) -> ControlResponse {
    request(d, 0x80, 0, 0, 0, 2, &[])
}

#[test]
fn get_descriptor_device_returns_the_device_descriptor() {
    for kind in [KEYBOARD, POINTER] {
        let (mut d, _) = addressed_device(kind);
        assert_eq!(
            request(&mut d, 0x80, 6, 0x0100, 0, 18, &[]),
            data(&device_descriptor(kind))
        );
    }
}

#[test]
fn get_descriptor_is_allowed_in_the_default_state() {
    let (mut d, _) = HidDevice::new(KEYBOARD);
    assert_eq!(
        request(&mut d, 0x80, 6, 0x0100, 0, 8, &[]),
        data(&device_descriptor(KEYBOARD))
    );
}

#[test]
fn get_descriptor_configuration_returns_the_whole_set() {
    for kind in [KEYBOARD, POINTER] {
        let (mut d, _) = addressed_device(kind);
        assert_eq!(
            request(&mut d, 0x80, 6, 0x0200, 0, 34, &[]),
            data(&configuration_descriptor(kind))
        );
    }
}

#[test]
fn get_descriptor_configuration_other_than_index_0_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x80, 6, 0x0201, 0, 34, &[]), STALL);
}

#[test]
fn get_descriptor_string_returns_the_language_list_and_both_strings() {
    let (mut d, _) = addressed_device(KEYBOARD);
    for index in 0..=2u16 {
        assert_eq!(
            request(&mut d, 0x80, 6, 0x0300 | index, 0x0409, 255, &[]),
            data(&string_descriptor(KEYBOARD, index as u8).unwrap())
        );
    }
}

#[test]
fn get_descriptor_string_with_an_unknown_index_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x80, 6, 0x0303, 0x0409, 255, &[]), STALL);
}

#[test]
fn get_descriptor_microsoft_os_string_0xee_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x80, 6, 0x03EE, 0, 255, &[]), STALL);
}

#[test]
fn get_descriptor_of_other_types_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    for descriptor_type in [0u16, 4, 5, 6, 7, 8, 15, 0x21, 0x22, 0x23] {
        assert_eq!(
            request(&mut d, 0x80, 6, descriptor_type << 8, 0, 255, &[]),
            STALL,
            "descriptor type {descriptor_type:#x} to the device"
        );
    }
}

#[test]
fn get_descriptor_hid_and_report_return_the_class_descriptors() {
    for kind in [KEYBOARD, POINTER] {
        let (mut d, _) = addressed_device(kind);
        assert_eq!(
            request(&mut d, 0x81, 6, 0x2100, 0, 9, &[]),
            data(&hid_descriptor(kind))
        );
        assert_eq!(
            request(&mut d, 0x81, 6, 0x2200, 0, 255, &[]),
            data(&report_descriptor(kind))
        );
    }
}

#[test]
fn get_descriptor_physical_to_the_interface_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x81, 6, 0x2300, 0, 255, &[]), STALL);
}

#[test]
fn get_descriptor_for_an_interface_other_than_0_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x81, 6, 0x2200, 1, 255, &[]), STALL);
}

#[test]
fn get_status_of_the_device_is_zero() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(get_status_device(&mut d), data(&[0, 0]));
}

#[test]
fn get_status_of_the_interface_stalls_in_the_address_state_and_is_zero_when_configured() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x81, 0, 0, 0, 2, &[]), STALL);
    assert_eq!(request(&mut d, 0x00, 9, 1, 0, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0x81, 0, 0, 0, 2, &[]), data(&[0, 0]));
}

#[test]
fn get_status_of_the_interrupt_endpoint_reports_the_halt_bit() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x82, 0, 0, 0x81, 2, &[]), data(&[0, 0]));
    assert_eq!(request(&mut d, 0x02, 3, 0, 0x81, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0x82, 0, 0, 0x81, 2, &[]), data(&[1, 0]));
}

#[test]
fn get_status_of_the_interrupt_endpoint_stalls_in_the_address_state() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x82, 0, 0, 0x81, 2, &[]), STALL);
}

#[test]
fn get_status_of_endpoint_0_is_zero_and_of_other_endpoints_stalls() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x82, 0, 0, 0x00, 2, &[]), data(&[0, 0]));
    assert_eq!(request(&mut d, 0x82, 0, 0, 0x02, 2, &[]), STALL);
    assert_eq!(request(&mut d, 0x82, 0, 0, 0x01, 2, &[]), STALL);
}

#[test]
fn set_feature_endpoint_halt_makes_the_interrupt_endpoint_stall_and_clear_feature_releases_it() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x02, 3, 0, 0x81, 0, &[]), ACK);
    assert_eq!(d.interrupt_in(), InterruptStatus::Stall);
    assert_eq!(request(&mut d, 0x02, 1, 0, 0x81, 0, &[]), ACK);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn endpoint_halt_features_stall_in_the_address_state() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x02, 3, 0, 0x81, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x02, 1, 0, 0x81, 0, &[]), STALL);
}

#[test]
fn endpoint_features_of_other_endpoints_or_features_stall() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x02, 3, 0, 0x01, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x02, 3, 0, 0x00, 0, &[]), STALL);
    assert_eq!(
        request(&mut d, 0x02, 3, 1, 0x81, 0, &[]),
        STALL,
        "feature other than ENDPOINT_HALT"
    );
}

#[test]
fn device_and_interface_features_stall() {
    let (mut d, _) = configured_device(KEYBOARD);
    // DEVICE_REMOTE_WAKEUP (1), TEST_MODE (2), and interface features.
    assert_eq!(request(&mut d, 0x00, 3, 1, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x00, 1, 1, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x00, 3, 2, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x01, 3, 0, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x01, 1, 0, 0, 0, &[]), STALL);
}

#[test]
fn get_configuration_is_0_in_the_address_state_and_1_when_configured() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x80, 8, 0, 0, 1, &[]), data(&[0]));
    assert_eq!(request(&mut d, 0x00, 9, 1, 0, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0x80, 8, 0, 0, 1, &[]), data(&[1]));
}

#[test]
fn set_configuration_0_returns_the_device_to_the_address_state() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x00, 9, 0, 0, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0x80, 8, 0, 0, 1, &[]), data(&[0]));
    assert_eq!(request(&mut d, 0x81, 10, 0, 0, 1, &[]), STALL);
}

#[test]
fn set_configuration_with_another_value_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x00, 9, 2, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x00, 9, 0x0101, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x80, 8, 0, 0, 1, &[]), data(&[0]));
}

#[test]
fn set_configuration_in_the_default_state_stalls() {
    let (mut d, _) = HidDevice::new(KEYBOARD);
    assert_eq!(request(&mut d, 0x00, 9, 1, 0, 0, &[]), STALL);
}

#[test]
fn set_configuration_resets_the_halt_and_drops_queued_reports() {
    let (mut d, h) = configured_device(KEYBOARD);
    d.input(&InputEvent::Key {
        usage: 0x04,
        pressed: true,
    });
    assert_eq!(h.queued_reports(), 1);
    assert_eq!(request(&mut d, 0x02, 3, 0, 0x81, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0x00, 9, 1, 0, 0, &[]), ACK);
    assert_eq!(h.queued_reports(), 0);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak, "no halt");
}

#[test]
fn get_interface_is_0_when_configured_and_stalls_in_the_address_state() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x81, 10, 0, 0, 1, &[]), STALL);
    request(&mut d, 0x00, 9, 1, 0, 0, &[]);
    assert_eq!(request(&mut d, 0x81, 10, 0, 0, 1, &[]), data(&[0]));
    assert_eq!(
        request(&mut d, 0x81, 10, 0, 1, 1, &[]),
        STALL,
        "interface 1 does not exist"
    );
}

#[test]
fn set_interface_accepts_only_alternate_setting_0_and_resets_the_halt() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(
        request(&mut d, 0x01, 11, 0, 0, 0, &[]),
        STALL,
        "Address state"
    );
    request(&mut d, 0x00, 9, 1, 0, 0, &[]);
    request(&mut d, 0x02, 3, 0, 0x81, 0, &[]);
    assert_eq!(request(&mut d, 0x01, 11, 1, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x01, 11, 0, 1, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x01, 11, 0, 0, 0, &[]), ACK);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak, "the halt is reset");
}

#[test]
fn set_address_reaching_the_device_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x00, 5, 7, 0, 0, &[]), STALL);
    assert_eq!(d.address(), 1);
}

#[test]
fn set_descriptor_and_synch_frame_stall() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x00, 7, 0x0100, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x82, 12, 0, 0x81, 2, &[]), STALL);
}

#[test]
fn unknown_requests_and_wrong_request_types_stall() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x80, 0x40, 0, 0, 0, &[]), STALL);
    assert_eq!(
        request(&mut d, 0xC0, 1, 0, 0, 0, &[]),
        STALL,
        "vendor request"
    );
    assert_eq!(
        request(&mut d, 0x00, 6, 0x0100, 0, 18, &[]),
        STALL,
        "GET_DESCRIPTOR OUT"
    );
    assert_eq!(
        request(&mut d, 0x80, 9, 1, 0, 0, &[]),
        STALL,
        "SET_CONFIGURATION IN"
    );
}

// ---- HID class requests

#[test]
fn get_report_input_returns_the_current_keyboard_state() {
    let (mut d, _) = addressed_device(KEYBOARD);
    d.input(&InputEvent::Key {
        usage: 0xE1,
        pressed: true,
    });
    d.input(&InputEvent::Key {
        usage: 0x04,
        pressed: true,
    });
    assert_eq!(
        request(&mut d, 0xA1, 1, 0x0100, 0, 8, &[]),
        data(&[0x02, 0, 4, 0, 0, 0, 0, 0])
    );
}

#[test]
fn get_report_output_returns_the_led_byte() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x21, 9, 0x0200, 0, 1, &[0x03]), ACK);
    assert_eq!(request(&mut d, 0xA1, 1, 0x0200, 0, 1, &[]), data(&[0x03]));
}

#[test]
fn get_report_feature_or_report_id_other_than_0_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0xA1, 1, 0x0300, 0, 1, &[]), STALL);
    assert_eq!(request(&mut d, 0xA1, 1, 0x0101, 0, 8, &[]), STALL);
    assert_eq!(request(&mut d, 0xA1, 1, 0x0000, 0, 8, &[]), STALL);
    assert_eq!(request(&mut d, 0xA1, 1, 0x0100, 1, 8, &[]), STALL);
}

#[test]
fn get_report_input_returns_the_current_pointer_state_without_the_wheel() {
    let (mut d, _) = addressed_device(POINTER);
    d.input(&InputEvent::PointerAbs {
        x: 0x1234,
        y: 0x7FFF,
    });
    d.input(&InputEvent::Buttons { mask: 0b101 });
    d.input(&InputEvent::Wheel { delta: 5 });
    assert_eq!(
        request(&mut d, 0xA1, 1, 0x0100, 0, 6, &[]),
        data(&[0b101, 0x34, 0x12, 0xFF, 0x7F, 0])
    );
}

#[test]
fn get_report_output_of_the_pointer_stalls() {
    let (mut d, _) = addressed_device(POINTER);
    assert_eq!(request(&mut d, 0xA1, 1, 0x0200, 0, 1, &[]), STALL);
}

#[test]
fn set_report_output_stores_the_led_bits_for_the_test_api() {
    let (mut d, h) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x21, 9, 0x0200, 0, 1, &[0x02]), ACK);
    assert_eq!(h.leds(), 0x02);
    assert_eq!(request(&mut d, 0x21, 9, 0x0200, 0, 1, &[0xFF]), ACK);
    assert_eq!(h.leds(), 0x1F, "bits 5-7 are zero");
}

#[test]
fn set_report_with_a_wrong_length_type_or_report_id_stalls() {
    let (mut d, h) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x21, 9, 0x0200, 0, 2, &[1, 2]), STALL);
    assert_eq!(
        request(&mut d, 0x21, 9, 0x0200, 0, 1, &[]),
        STALL,
        "no data"
    );
    assert_eq!(
        request(&mut d, 0x21, 9, 0x0100, 0, 1, &[1]),
        STALL,
        "Input report"
    );
    assert_eq!(
        request(&mut d, 0x21, 9, 0x0300, 0, 1, &[1]),
        STALL,
        "Feature report"
    );
    assert_eq!(
        request(&mut d, 0x21, 9, 0x0201, 0, 1, &[1]),
        STALL,
        "Report ID 1"
    );
    assert_eq!(h.leds(), 0);
}

#[test]
fn set_report_of_the_pointer_stalls() {
    let (mut d, _) = addressed_device(POINTER);
    assert_eq!(request(&mut d, 0x21, 9, 0x0200, 0, 1, &[1]), STALL);
}

#[test]
fn idle_is_stored_and_returned() {
    for kind in [KEYBOARD, POINTER] {
        let (mut d, _) = addressed_device(kind);
        assert_eq!(
            request(&mut d, 0xA1, 2, 0, 0, 1, &[]),
            data(&[0]),
            "reset value 0"
        );
        assert_eq!(request(&mut d, 0x21, 0x0A, 0x7D00, 0, 0, &[]), ACK);
        assert_eq!(request(&mut d, 0xA1, 2, 0, 0, 1, &[]), data(&[0x7D]));
    }
}

#[test]
fn set_idle_for_a_report_id_other_than_0_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x21, 0x0A, 0x0101, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0xA1, 2, 0x0001, 0, 1, &[]), STALL);
}

#[test]
fn protocol_of_the_keyboard_defaults_to_report_and_can_be_set() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0xA1, 3, 0, 0, 1, &[]), data(&[1]));
    assert_eq!(request(&mut d, 0x21, 0x0B, 0, 0, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0xA1, 3, 0, 0, 1, &[]), data(&[0]));
    assert_eq!(request(&mut d, 0x21, 0x0B, 1, 0, 0, &[]), ACK);
    assert_eq!(request(&mut d, 0xA1, 3, 0, 0, 1, &[]), data(&[1]));
}

#[test]
fn set_protocol_with_another_value_stalls() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0x21, 0x0B, 2, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0xA1, 3, 0, 0, 1, &[]), data(&[1]));
}

#[test]
fn protocol_requests_of_the_pointer_stall() {
    let (mut d, _) = addressed_device(POINTER);
    assert_eq!(request(&mut d, 0xA1, 3, 0, 0, 1, &[]), STALL);
    assert_eq!(request(&mut d, 0x21, 0x0B, 0, 0, 0, &[]), STALL);
}

#[test]
fn unknown_class_requests_stall() {
    let (mut d, _) = addressed_device(KEYBOARD);
    assert_eq!(request(&mut d, 0xA1, 0x04, 0, 0, 0, &[]), STALL);
    assert_eq!(request(&mut d, 0x21, 0x0C, 0, 0, 0, &[]), STALL);
}

#[test]
fn a_reset_returns_the_device_to_the_default_state() {
    let (mut d, h) = configured_device(KEYBOARD);
    request(&mut d, 0x21, 0x0A, 0x1000, 0, 0, &[]);
    request(&mut d, 0x21, 0x0B, 0, 0, 0, &[]);
    request(&mut d, 0x21, 9, 0x0200, 0, 1, &[1]);
    request(&mut d, 0x02, 3, 0, 0x81, 0, &[]);
    d.input(&InputEvent::Key {
        usage: 0x04,
        pressed: true,
    });
    d.reset();
    assert_eq!(d.address(), 0);
    assert_eq!(h.queued_reports(), 0);
    assert_eq!(h.leds(), 0);
    d.set_address(1);
    assert_eq!(
        request(&mut d, 0x80, 8, 0, 0, 1, &[]),
        data(&[0]),
        "not configured"
    );
    assert_eq!(request(&mut d, 0xA1, 2, 0, 0, 1, &[]), data(&[0]), "idle 0");
    assert_eq!(
        request(&mut d, 0xA1, 3, 0, 0, 1, &[]),
        data(&[1]),
        "Report protocol"
    );
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak, "no halt");
    // A bus reset leaves the keys down...
    assert_eq!(
        request(&mut d, 0xA1, 1, 0x0100, 0, 8, &[]),
        data(&[0, 0, 4, 0, 0, 0, 0, 0])
    );
}

#[test]
fn hcrst_also_releases_keys_buttons_and_keeps_the_coordinates() {
    let (mut k, _) = addressed_device(KEYBOARD);
    k.input(&InputEvent::Key {
        usage: 0xE0,
        pressed: true,
    });
    k.input(&InputEvent::Key {
        usage: 0x04,
        pressed: true,
    });
    k.host_controller_reset();
    k.set_address(1);
    assert_eq!(request(&mut k, 0xA1, 1, 0x0100, 0, 8, &[]), data(&[0; 8]));
    let (mut p, _) = addressed_device(POINTER);
    p.input(&InputEvent::PointerAbs { x: 100, y: 200 });
    p.input(&InputEvent::Buttons { mask: 1 });
    p.host_controller_reset();
    p.set_address(1);
    assert_eq!(
        request(&mut p, 0xA1, 1, 0x0100, 0, 6, &[]),
        data(&[0, 100, 0, 200, 0, 0])
    );
}

// ---------------------------------------------------------------- keyboard input

fn key(d: &mut HidDevice, usage: u8, pressed: bool) {
    d.input(&InputEvent::Key { usage, pressed });
}

fn next_report(d: &mut HidDevice) -> Vec<u8> {
    match d.interrupt_in() {
        InterruptStatus::Report(length) => {
            let report = d.take_interrupt_report();
            assert_eq!(report.len(), length);
            report
        }
        other => panic!("no report: {other:?}"),
    }
}

#[test]
fn a_key_press_and_release_give_one_report_each() {
    let (mut d, _) = configured_device(KEYBOARD);
    key(&mut d, 0x04, true);
    key(&mut d, 0x04, false);
    assert_eq!(next_report(&mut d), [0, 0, 4, 0, 0, 0, 0, 0]);
    assert_eq!(next_report(&mut d), [0; 8]);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn keys_are_reported_in_the_order_they_were_pressed() {
    let (mut d, _) = configured_device(KEYBOARD);
    key(&mut d, 0x1D, true);
    key(&mut d, 0x04, true);
    key(&mut d, 0x10, true);
    next_report(&mut d);
    next_report(&mut d);
    assert_eq!(next_report(&mut d), [0, 0, 0x1D, 0x04, 0x10, 0, 0, 0]);
    key(&mut d, 0x04, false);
    assert_eq!(next_report(&mut d), [0, 0, 0x1D, 0x10, 0, 0, 0, 0]);
}

#[test]
fn modifiers_go_to_byte_0_by_usage_id() {
    let (mut d, _) = configured_device(KEYBOARD);
    for (usage, bit) in [
        (0xE0, 0x01),
        (0xE1, 0x02),
        (0xE2, 0x04),
        (0xE3, 0x08),
        (0xE4, 0x10),
        (0xE5, 0x20),
        (0xE6, 0x40),
        (0xE7, 0x80),
    ] {
        key(&mut d, usage, true);
        assert_eq!(next_report(&mut d)[0] & bit, bit, "usage {usage:#x}");
        key(&mut d, usage, false);
        assert_eq!(next_report(&mut d)[0], 0);
    }
}

#[test]
fn a_repeated_press_or_a_release_of_a_key_that_is_up_gives_no_report() {
    let (mut d, _) = configured_device(KEYBOARD);
    key(&mut d, 0x05, false);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
    key(&mut d, 0x05, true);
    key(&mut d, 0x05, true);
    key(&mut d, 0xE1, true);
    key(&mut d, 0xE1, true);
    key(&mut d, 0xE2, false);
    next_report(&mut d);
    next_report(&mut d);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn usage_ids_outside_the_key_ranges_are_dropped_and_counted() {
    let (mut d, h) = configured_device(KEYBOARD);
    for usage in [0x00, 0x01, 0x02, 0x03, 0xDE, 0xDF, 0xE8, 0xFF] {
        key(&mut d, usage, true);
    }
    assert_eq!(h.dropped_events(), 8);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn the_first_and_last_key_usage_ids_are_accepted() {
    let (mut d, h) = configured_device(KEYBOARD);
    key(&mut d, 0x04, true);
    key(&mut d, 0xDD, true);
    assert_eq!(h.dropped_events(), 0);
    next_report(&mut d);
    assert_eq!(next_report(&mut d), [0, 0, 0x04, 0xDD, 0, 0, 0, 0]);
}

#[test]
fn six_keys_fit_and_the_seventh_gives_error_roll_over_until_one_is_released() {
    let (mut d, _) = configured_device(KEYBOARD);
    key(&mut d, 0xE1, true);
    for usage in 0x04..=0x09 {
        key(&mut d, usage, true);
    }
    let reports: Vec<Vec<u8>> = (0..7).map(|_| next_report(&mut d)).collect();
    assert_eq!(reports[6], [0x02, 0, 4, 5, 6, 7, 8, 9]);
    key(&mut d, 0x0A, true);
    assert_eq!(
        next_report(&mut d),
        [0x02, 0, 1, 1, 1, 1, 1, 1],
        "modifiers are kept"
    );
    key(&mut d, 0x0A, false);
    assert_eq!(next_report(&mut d), [0x02, 0, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn keys_pressed_while_the_endpoint_is_not_running_update_the_state_without_reports() {
    let (mut d, h) = addressed_device(KEYBOARD);
    key(&mut d, 0x04, true);
    assert_eq!(h.queued_reports(), 0, "not configured");
    request(&mut d, 0x00, 9, 1, 0, 0, &[]);
    key(&mut d, 0x05, true);
    assert_eq!(h.queued_reports(), 0, "endpoint not running");
    d.set_interrupt_endpoint_active(true);
    assert_eq!(
        d.interrupt_in(),
        InterruptStatus::Nak,
        "nothing is sent on configuration"
    );
    key(&mut d, 0x06, true);
    assert_eq!(
        next_report(&mut d),
        [0, 0, 4, 5, 6, 0, 0, 0],
        "the full state"
    );
}

#[test]
fn stopping_the_endpoint_drops_queued_reports() {
    let (mut d, h) = configured_device(KEYBOARD);
    key(&mut d, 0x04, true);
    assert_eq!(h.queued_reports(), 1);
    d.set_interrupt_endpoint_active(false);
    assert_eq!(h.queued_reports(), 0);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn an_empty_queue_naks() {
    let (mut d, _) = configured_device(KEYBOARD);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
    assert_eq!(d.take_interrupt_report(), Vec::<u8>::new());
}

#[test]
fn the_report_queue_holds_16_and_a_full_queue_replaces_the_last_report() {
    let (mut d, h) = configured_device(KEYBOARD);
    for usage in 0x04..0x04 + 16 {
        key(&mut d, usage, true);
    }
    assert_eq!(h.queued_reports(), 16);
    assert_eq!(h.reports_coalesced(), 0);
    key(&mut d, 0x30, true);
    key(&mut d, 0x31, true);
    assert_eq!(h.queued_reports(), 16);
    assert_eq!(h.reports_coalesced(), 2);
    let reports: Vec<Vec<u8>> = (0..16).map(|_| next_report(&mut d)).collect();
    // Seventeen keys are down: the last report is the snapshot after the 18th key, which is a roll over.
    assert_eq!(
        reports[14][2], 0x01,
        "ErrorRollOver from the seventh key on"
    );
    assert_eq!(reports[15], [0, 0, 1, 1, 1, 1, 1, 1]);
}

#[test]
fn coalescing_keeps_the_final_keyboard_state() {
    let (mut d, h) = configured_device(KEYBOARD);
    for _ in 0..8 {
        key(&mut d, 0x04, true);
        key(&mut d, 0x04, false);
    }
    assert_eq!(h.queued_reports(), 16);
    key(&mut d, 0x05, true);
    assert_eq!(h.reports_coalesced(), 1);
    let reports: Vec<Vec<u8>> = (0..16).map(|_| next_report(&mut d)).collect();
    assert_eq!(reports[15], [0, 0, 5, 0, 0, 0, 0, 0]);
}

#[test]
fn the_keyboard_does_not_repeat_reports_for_a_nonzero_idle() {
    let (mut d, _) = configured_device(KEYBOARD);
    request(&mut d, 0x21, 0x0A, 0x7D00, 0, 0, &[]);
    key(&mut d, 0x04, true);
    next_report(&mut d);
    for _ in 0..1000 {
        assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
    }
}

#[test]
fn the_keyboard_ignores_pointer_events() {
    let (mut d, h) = configured_device(KEYBOARD);
    d.input(&InputEvent::PointerAbs { x: 1, y: 2 });
    d.input(&InputEvent::Buttons { mask: 1 });
    d.input(&InputEvent::Wheel { delta: 1 });
    assert_eq!(h.queued_reports(), 0);
    assert_eq!(h.dropped_events(), 0);
}

// ---------------------------------------------------------------- pointer input

#[test]
fn the_pointer_reports_absolute_coordinates_little_endian() {
    let (mut d, _) = configured_device(POINTER);
    d.input(&InputEvent::PointerAbs {
        x: 0x1234,
        y: 0x0567,
    });
    assert_eq!(next_report(&mut d), [0, 0x34, 0x12, 0x67, 0x05, 0]);
}

#[test]
fn the_pointer_accepts_the_coordinate_range_boundaries() {
    let (mut d, h) = configured_device(POINTER);
    d.input(&InputEvent::PointerAbs { x: 32767, y: 32767 });
    assert_eq!(next_report(&mut d), [0, 0xFF, 0x7F, 0xFF, 0x7F, 0]);
    d.input(&InputEvent::PointerAbs { x: 0, y: 0 });
    assert_eq!(next_report(&mut d), [0; 6]);
    assert_eq!(h.dropped_events(), 0);
}

#[test]
fn the_pointer_drops_coordinates_above_32767() {
    let (mut d, h) = configured_device(POINTER);
    d.input(&InputEvent::PointerAbs { x: 32768, y: 0 });
    d.input(&InputEvent::PointerAbs { x: 0, y: 32768 });
    d.input(&InputEvent::PointerAbs {
        x: 0xFFFF,
        y: 0xFFFF,
    });
    assert_eq!(h.dropped_events(), 3);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
    d.input(&InputEvent::PointerAbs { x: 5, y: 6 });
    assert_eq!(
        next_report(&mut d),
        [0, 5, 0, 6, 0, 0],
        "the old position was kept"
    );
}

#[test]
fn a_pointer_move_to_the_same_place_gives_no_report() {
    let (mut d, _) = configured_device(POINTER);
    d.input(&InputEvent::PointerAbs { x: 7, y: 8 });
    next_report(&mut d);
    d.input(&InputEvent::PointerAbs { x: 7, y: 8 });
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn pointer_buttons_use_bits_0_to_2_and_ignore_the_rest() {
    let (mut d, _) = configured_device(POINTER);
    d.input(&InputEvent::Buttons { mask: 0b1111_1101 });
    assert_eq!(next_report(&mut d)[0], 0b101);
    d.input(&InputEvent::Buttons { mask: 0b1111_1000 });
    assert_eq!(next_report(&mut d)[0], 0);
    d.input(&InputEvent::Buttons { mask: 0b1111_1000 });
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak, "no change");
}

#[test]
fn the_wheel_is_a_signed_increment_since_the_last_report() {
    let (mut d, _) = configured_device(POINTER);
    d.input(&InputEvent::Wheel { delta: 3 });
    assert_eq!(next_report(&mut d)[5], 3);
    d.input(&InputEvent::Wheel { delta: -127 });
    assert_eq!(next_report(&mut d)[5], 0x81);
    d.input(&InputEvent::Wheel { delta: 127 });
    assert_eq!(next_report(&mut d)[5], 127);
    d.input(&InputEvent::Buttons { mask: 1 });
    assert_eq!(
        next_report(&mut d)[5],
        0,
        "no movement since the last report"
    );
}

#[test]
fn a_wheel_of_minus_128_is_out_of_range_and_dropped() {
    let (mut d, h) = configured_device(POINTER);
    d.input(&InputEvent::Wheel { delta: -128 });
    assert_eq!(h.dropped_events(), 1);
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn a_zero_wheel_gives_no_report() {
    let (mut d, _) = configured_device(POINTER);
    d.input(&InputEvent::Wheel { delta: 0 });
    assert_eq!(d.interrupt_in(), InterruptStatus::Nak);
}

#[test]
fn wheel_movement_without_a_reader_is_not_kept() {
    let (mut d, _) = addressed_device(POINTER);
    request(&mut d, 0x00, 9, 1, 0, 0, &[]);
    d.input(&InputEvent::Wheel { delta: 100 });
    d.set_interrupt_endpoint_active(true);
    d.input(&InputEvent::Buttons { mask: 1 });
    assert_eq!(next_report(&mut d)[5], 0, "no phantom scroll");
    d.set_interrupt_endpoint_active(false);
    d.set_interrupt_endpoint_active(true);
    d.input(&InputEvent::Wheel { delta: 7 });
    assert_eq!(next_report(&mut d)[5], 7);
}

#[test]
fn a_wheel_remainder_beyond_127_is_sent_in_portions() {
    let (mut d, h) = configured_device(POINTER);
    // A full queue merges the wheel movement of replaced reports: 3 x 100 does not fit a byte.
    for x in 1..=16 {
        d.input(&InputEvent::PointerAbs { x, y: 0 });
    }
    for _ in 0..3 {
        d.input(&InputEvent::Wheel { delta: 100 });
    }
    assert_eq!(h.reports_coalesced(), 3);
    let reports: Vec<Vec<u8>> = (0..16).map(|_| next_report(&mut d)).collect();
    assert_eq!(reports[15][5], 127, "at most 127 per report");
    d.input(&InputEvent::Buttons { mask: 1 });
    assert_eq!(next_report(&mut d)[5], 127);
    d.input(&InputEvent::Buttons { mask: 0 });
    assert_eq!(next_report(&mut d)[5], 46, "the rest of 300");
}

#[test]
fn coalescing_pointer_reports_keeps_the_wheel_movement() {
    let (mut d, h) = configured_device(POINTER);
    for x in 1..=16 {
        d.input(&InputEvent::PointerAbs { x, y: 0 });
    }
    d.input(&InputEvent::Wheel { delta: 5 });
    d.input(&InputEvent::Wheel { delta: 6 });
    assert_eq!(h.reports_coalesced(), 2);
    let reports: Vec<Vec<u8>> = (0..16).map(|_| next_report(&mut d)).collect();
    assert_eq!(reports[15][5], 11, "the replaced report's wheel is merged");
}

#[test]
fn the_pointer_ignores_keyboard_events() {
    let (mut d, h) = configured_device(POINTER);
    d.input(&InputEvent::Key {
        usage: 0x04,
        pressed: true,
    });
    d.input(&InputEvent::Key {
        usage: 0x00,
        pressed: true,
    });
    assert_eq!(h.queued_reports(), 0);
    assert_eq!(h.dropped_events(), 0);
}

// ---------------------------------------------------------------- through the controller

fn hid_fixture(port: u32) -> (Fixture, HidDevices) {
    let mut devices = None;
    let fixture = addressed_with(port, EP0_RING, |xhci, _| {
        devices = Some(xhci.attach_hid_devices());
    });
    (fixture, devices.unwrap())
}

use super::transfers::Fixture;

/// Runs one control TD at the next free place of the EP0 ring.
struct Control {
    next: u64,
}

impl Control {
    fn new() -> Self {
        Self { next: EP0_RING }
    }

    fn run(&mut self, f: &mut Fixture, trbs: &[[u32; 4]]) -> Vec<Event> {
        f.rig.put_ring(self.next, trbs);
        self.next += 16 * trbs.len() as u64;
        f.ring(EP0);
        f.rig.events()
    }
}

#[test]
fn the_keyboard_enumerates_and_delivers_a_report_through_the_controller() {
    let (mut f, devices) = hid_fixture(1);
    let mut control = Control::new();
    // GET_DESCRIPTOR(device, 8 bytes) first, as drivers do, then all 18.
    let events = control.run(
        &mut f,
        &[
            get_descriptor(8, 0),
            data_trb(BUF, 8, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(events[0].code, CC_SUCCESS);
    assert_eq!(f.rig.mem_read(BUF, 8), device_descriptor(KEYBOARD)[..8]);
    let events = control.run(
        &mut f,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(events[0].code, CC_SUCCESS);
    assert_eq!(f.rig.mem_read(BUF, 18), device_descriptor(KEYBOARD));
    // The 34 byte configuration set in a chain that crosses a page boundary.
    let events = control.run(
        &mut f,
        &[
            setup_trb(0x80, 6, 0x0200, 0, 34, 3, 0),
            data_trb(BUF + 0xFF0, 16, true, CH),
            normal_trb(BUF2, 18, IOC),
            status_trb(false, IOC),
        ],
    );
    assert_eq!(events.len(), 2);
    let mut config = f.rig.mem_read(BUF + 0xFF0, 16);
    config.extend(f.rig.mem_read(BUF2, 18));
    assert_eq!(config, configuration_descriptor(KEYBOARD));
    // Report descriptor through the interface.
    let events = control.run(
        &mut f,
        &[
            setup_trb(0x81, 6, 0x2200, 0, 64, 3, 0),
            data_trb(BUF, 64, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(events[0].code, CC_SUCCESS);
    assert_eq!(f.rig.mem_read(BUF, 64), KEYBOARD_REPORT_DESCRIPTOR);
    // SET_CONFIGURATION(1), then the controller side Configure Endpoint.
    control.run(&mut f, &[set_configuration(0), status_trb(true, IOC)]);
    let mut f = add_interrupt_endpoint(f, 1);
    f.rig.put_ring(
        INTERRUPT_RING,
        &[normal_trb(BUF, 8, IOC), normal_trb(BUF + 0x100, 8, IOC)],
    );
    f.ring(EP3);
    assert!(f.rig.events().is_empty(), "no report yet");
    assert!(f.rig.xhci.service_pending(), "the endpoint polls");
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x04,
            pressed: true,
        })
        .unwrap();
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x04,
            pressed: false,
        })
        .unwrap();
    f.rig.service();
    let events = f.rig.events();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| e.code == CC_SUCCESS && e.kind == TRB_TYPE_TRANSFER_EVENT)
    );
    assert_eq!(f.rig.mem_read(BUF, 8), [0, 0, 4, 0, 0, 0, 0, 0]);
    assert_eq!(f.rig.mem_read(BUF + 0x100, 8), [0; 8]);
    assert_eq!(devices.keyboard.queued_reports(), 0);
}

#[test]
fn the_pointer_enumerates_and_delivers_a_six_byte_report() {
    let (mut f, devices) = hid_fixture(2);
    let mut control = Control::new();
    control.run(
        &mut f,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(f.rig.mem_read(BUF, 18), device_descriptor(POINTER));
    control.run(&mut f, &[set_configuration(0), status_trb(true, 0)]);
    let mut f = add_interrupt_endpoint(f, 2);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 6, IOC)]);
    f.ring(EP3);
    f.rig
        .xhci
        .push_input(InputEvent::PointerAbs { x: 32767, y: 1 })
        .unwrap();
    f.rig.service();
    assert_eq!(f.rig.events().len(), 1);
    assert_eq!(f.rig.mem_read(BUF, 6), [0, 0xFF, 0x7F, 1, 0, 0]);
    assert_eq!(devices.pointer.dropped_events(), 0);
}

#[test]
fn the_microsoft_os_string_stalls_ep0_which_recovers_with_reset_endpoint() {
    let (mut f, _devices) = hid_fixture(1);
    let mut control = Control::new();
    let events = control.run(
        &mut f,
        &[
            setup_trb(0x80, 6, 0x03EE, 0, 255, 3, 0),
            data_trb(BUF, 255, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(events[0].code, CC_STALL_ERROR);
    assert_eq!(f.state(EP0), EndpointState::Halted);
    f.rig
        .command_ok(endpoint_command(TRB_TYPE_RESET_ENDPOINT, f.slot, EP0));
    control.next += 0x100;
    f.rig
        .command_ok(set_dequeue_command(f.slot, EP0, control.next));
    let events = control.run(
        &mut f,
        &[
            get_descriptor(18, 0),
            data_trb(BUF, 18, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(events[0].code, CC_SUCCESS);
    assert_eq!(f.rig.mem_read(BUF, 18), device_descriptor(KEYBOARD));
}

#[test]
fn set_protocol_set_idle_and_set_report_reach_the_keyboard_through_ep0() {
    let (mut f, devices) = hid_fixture(1);
    let mut control = Control::new();
    f.rig.mem_write(BUF, &[0x05]);
    let events = control.run(
        &mut f,
        &[
            setup_trb(0x21, 9, 0x0200, 0, 1, 2, 0),
            data_trb(BUF, 1, false, IOC),
            status_trb(true, IOC),
        ],
    );
    assert_eq!(events.len(), 2);
    assert_eq!(devices.keyboard.leds(), 0x05);
    // SET_PROTOCOL(boot) and GET_PROTOCOL.
    control.run(
        &mut f,
        &[setup_trb(0x21, 0x0B, 0, 0, 0, 0, 0), status_trb(true, IOC)],
    );
    control.run(
        &mut f,
        &[
            setup_trb(0xA1, 0x03, 0, 0, 1, 3, 0),
            data_trb(BUF + 0x10, 1, true, IOC),
            status_trb(false, 0),
        ],
    );
    assert_eq!(f.rig.mem_read(BUF + 0x10, 1), [0]);
}

#[test]
fn input_before_configuration_updates_the_state_but_sends_no_report() {
    let (mut f, _devices) = hid_fixture(1);
    let mut control = Control::new();
    control.run(&mut f, &[set_configuration(0), status_trb(true, 0)]);
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x04,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    let mut f = add_interrupt_endpoint(f, 1);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 8, IOC)]);
    f.ring(EP3);
    assert!(
        f.rig.events().is_empty(),
        "nothing is sent on configuration"
    );
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x05,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    assert_eq!(f.rig.events().len(), 1);
    assert_eq!(
        f.rig.mem_read(BUF, 8),
        [0, 0, 4, 5, 0, 0, 0, 0],
        "the full state"
    );
}

#[test]
fn stop_endpoint_drops_the_queued_reports_of_the_device() {
    let (mut f, devices) = hid_fixture(1);
    let mut control = Control::new();
    control.run(&mut f, &[set_configuration(0), status_trb(true, 0)]);
    let mut f = add_interrupt_endpoint(f, 1);
    f.rig.service();
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x04,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    assert_eq!(
        devices.keyboard.queued_reports(),
        1,
        "no TRB queued: the report waits"
    );
    f.rig
        .push_command(endpoint_command(TRB_TYPE_STOP_ENDPOINT, f.slot, EP3));
    f.rig.ring_command_doorbell();
    f.rig.service();
    assert_eq!(
        devices.keyboard.queued_reports(),
        0,
        "dropped by the service call that stopped the endpoint"
    );
}

#[test]
fn the_input_queue_holds_256_events_and_refuses_the_next_one() {
    let (f, _devices) = hid_fixture(1);
    for _ in 0..256 {
        f.rig
            .xhci
            .push_input(InputEvent::Wheel { delta: 1 })
            .unwrap();
    }
    assert_eq!(
        f.rig.xhci.push_input(InputEvent::Wheel { delta: 1 }),
        Err(InputQueueFull)
    );
}

#[test]
fn one_service_call_hands_over_at_most_64_input_events() {
    let (mut f, devices) = hid_fixture(1);
    for _ in 0..100 {
        // Usage 0 is dropped and counted by the keyboard: a way to see how many arrived.
        f.rig
            .xhci
            .push_input(InputEvent::Key {
                usage: 0,
                pressed: true,
            })
            .unwrap();
    }
    assert!(f.rig.xhci.service_pending());
    f.rig.service();
    assert_eq!(devices.keyboard.dropped_events(), 64);
    assert!(f.rig.xhci.service_pending(), "the rest is waiting");
    f.rig.service();
    assert_eq!(devices.keyboard.dropped_events(), 100);
    f.rig.service();
    assert!(!f.rig.xhci.service_pending(), "everything was handed over");
}

#[test]
fn input_is_handed_over_without_bus_master() {
    let (mut f, devices) = hid_fixture(1);
    f.rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE);
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    assert_eq!(devices.keyboard.dropped_events(), 1);
}

#[test]
fn hcrst_clears_the_input_queue_and_releases_the_keys() {
    let (mut f, devices) = hid_fixture(1);
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x04,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0,
            pressed: true,
        })
        .unwrap();
    f.rig.set_dword(USBCMD, HCRST);
    f.rig.service();
    assert_eq!(
        devices.keyboard.dropped_events(),
        0,
        "the queued event was discarded"
    );
}

#[test]
fn attaching_a_device_to_a_port_that_does_not_exist_is_refused() {
    let f = hid_fixture(1).0;
    let device = TestDevice::default();
    assert!(f.rig.xhci.attach_device(3, Box::new(device)).is_err());
}

#[test]
fn set_interface_drops_queued_reports() {
    let (mut d, h) = configured_device(KEYBOARD);
    key(&mut d, 0x04, true);
    assert_eq!(h.queued_reports(), 1);
    assert_eq!(request(&mut d, 0x01, 11, 0, 0, 0, &[]), ACK);
    assert_eq!(h.queued_reports(), 0);
}

#[test]
fn a_device_holds_back_input_while_its_report_queue_is_full() {
    let (mut d, _) = configured_device(KEYBOARD);
    let event = InputEvent::Key {
        usage: 0x04,
        pressed: true,
    };
    assert!(d.input_ready(&event));
    for usage in 0x04..0x04 + 16 {
        key(&mut d, usage, true);
    }
    assert!(!d.input_ready(&event), "the queue is full");
    assert!(
        d.input_ready(&InputEvent::Wheel { delta: 1 }),
        "not its event"
    );
    d.set_interrupt_endpoint_active(false);
    assert!(d.input_ready(&event), "nobody reads: no queue to protect");
}

#[test]
fn a_burst_of_input_waits_in_the_machine_queue_instead_of_being_merged() {
    let (mut f, devices) = hid_fixture(1);
    let mut control = Control::new();
    control.run(&mut f, &[set_configuration(0), status_trb(true, 0)]);
    let mut f = add_interrupt_endpoint(f, 1);
    f.rig.service();
    // 40 events (20 keys pressed and released), nobody reads the interrupt endpoint yet.
    for usage in 4..24 {
        f.rig
            .xhci
            .push_input(InputEvent::Key {
                usage,
                pressed: true,
            })
            .unwrap();
        f.rig
            .xhci
            .push_input(InputEvent::Key {
                usage,
                pressed: false,
            })
            .unwrap();
    }
    f.rig.service();
    assert_eq!(devices.keyboard.queued_reports(), 16);
    assert!(
        !f.rig.xhci.service_pending(),
        "nothing more can be handed over"
    );
    // The driver queues 16 TRBs: every report arrives, in order.
    let trbs: Vec<[u32; 4]> = (0..16).map(|i| normal_trb(BUF + 0x100 * i, 8, 0)).collect();
    f.rig.put_ring(INTERRUPT_RING, &trbs);
    f.ring(EP3);
    assert_eq!(f.rig.mem_read(BUF, 8), [0, 0, 4, 0, 0, 0, 0, 0]);
    assert_eq!(f.rig.mem_read(BUF + 0x100, 8), [0; 8]);
    assert_eq!(f.rig.mem_read(BUF + 0x200, 8), [0, 0, 5, 0, 0, 0, 0, 0]);
    assert!(
        f.rig.xhci.service_pending(),
        "the rest of the input is now deliverable"
    );
    f.rig.service();
    assert_eq!(devices.keyboard.queued_reports(), 16);
    f.rig.service();
    assert_eq!(
        devices.keyboard.reports_coalesced(),
        0,
        "nothing was merged"
    );
}

#[test]
fn a_halted_endpoint_keeps_its_reports_until_the_reset_endpoint_command() {
    let (mut f, devices) = hid_fixture(1);
    let mut control = Control::new();
    control.run(&mut f, &[set_configuration(0), status_trb(true, 0)]);
    let mut f = add_interrupt_endpoint(f, 1);
    f.rig.put_ring(INTERRUPT_RING, &[normal_trb(BUF, 4, IOC)]);
    f.ring(EP3);
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x04,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    assert_eq!(f.state(EP3), EndpointState::Halted);
    f.rig.events();
    f.rig.service();
    assert_eq!(
        devices.keyboard.queued_reports(),
        1,
        "the report survives the halt"
    );
    f.rig
        .command_ok(endpoint_command(TRB_TYPE_RESET_ENDPOINT, f.slot, EP3));
    assert_eq!(
        devices.keyboard.queued_reports(),
        0,
        "a Stopped endpoint holds no reports: the next change sends the full state"
    );
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x05,
            pressed: true,
        })
        .unwrap();
    f.rig
        .command_ok(set_dequeue_command(f.slot, EP3, INTERRUPT_RING + 0x100));
    f.rig
        .put_ring(INTERRUPT_RING + 0x100, &[normal_trb(BUF2, 8, IOC)]);
    f.ring(EP3);
    f.rig
        .xhci
        .push_input(InputEvent::Key {
            usage: 0x06,
            pressed: true,
        })
        .unwrap();
    f.rig.service();
    assert_eq!(
        f.rig.mem_read(BUF2, 8),
        [0, 0, 4, 5, 6, 0, 0, 0],
        "the full state"
    );
}
