//! The USB HID devices of the board: a boot keyboard on port 1 and an absolute pointer on port 2
//! (`docs/specs/XHCI_DEVICE.md` section 10). They implement [`UsbDevice`]: standard requests
//! (USB 2.0 chapter 9), HID class requests (HID 1.11 section 7.2), and an Interrupt IN report
//! queue fed by input events.

use super::device::{ControlResponse, InputEvent, InterruptStatus, UsbDevice};
use super::hid_descriptors::{
    HidKind, configuration_descriptor, device_descriptor, hid_descriptor, report_descriptor,
    string_descriptor,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Reports an endpoint keeps (contract section 10.5).
const REPORT_QUEUE_LIMIT: usize = 16;
/// Largest absolute coordinate of the pointer (Logical Maximum of X and Y).
const POINTER_MAX: u16 = 32_767;
/// ErrorRollOver, reported in every key byte when more than six keys are down.
const ERROR_ROLL_OVER: u8 = 0x01;
/// LED bits of the keyboard Output report: NumLock, CapsLock, ScrollLock, Compose, Kana.
const LED_MASK: u8 = 0x1F;

const PROTOCOL_BOOT: u8 = 0;
const PROTOCOL_REPORT: u8 = 1;

const REPORT_TYPE_INPUT: u8 = 1;
const REPORT_TYPE_OUTPUT: u8 = 2;

/// Host-side view of a device: the test API of contract section 16 (LEDs, counters).
#[derive(Clone, Debug)]
pub struct HidHandle(Arc<Mutex<HidState>>);

impl HidHandle {
    /// The LED bits the host last set with SET_REPORT (keyboard; 0 for the pointer).
    pub fn leds(&self) -> u8 {
        self.with(|state| state.leds)
    }

    /// Reports that replaced the last queued one because the queue was full.
    pub fn reports_coalesced(&self) -> u64 {
        self.with(|state| state.coalesced)
    }

    /// Input events the device dropped: a Usage ID or coordinate out of range.
    pub fn dropped_events(&self) -> u64 {
        self.with(|state| state.dropped)
    }

    /// Reports waiting for an Interrupt IN transfer.
    pub fn queued_reports(&self) -> usize {
        self.with(|state| state.queue.len())
    }

    fn with<R>(&self, f: impl FnOnce(&mut HidState) -> R) -> R {
        f(&mut self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

/// The keyboard or the pointer.
#[derive(Debug)]
pub(super) struct HidDevice(Arc<Mutex<HidState>>);

impl HidDevice {
    pub(super) fn new(kind: HidKind) -> (Self, HidHandle) {
        let state = Arc::new(Mutex::new(HidState::new(kind)));
        (Self(Arc::clone(&state)), HidHandle(state))
    }

    fn with<R>(&self, f: impl FnOnce(&mut HidState) -> R) -> R {
        f(&mut self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

#[derive(Debug)]
struct HidState {
    kind: HidKind,
    address: u8,
    /// SET_CONFIGURATION(1) was done.
    configured: bool,
    /// ENDPOINT_HALT of the interrupt endpoint 0x81.
    halted: bool,
    /// The host's Interrupt IN endpoint is Running (reported by the controller).
    active: bool,
    idle: u8,
    protocol: u8,
    leds: u8,
    modifiers: u8,
    /// Non-modifier keys down, in the order they were pressed.
    keys: Vec<u8>,
    buttons: u8,
    x: u16,
    y: u16,
    /// Wheel movement not yet reported.
    wheel: i32,
    queue: VecDeque<Vec<u8>>,
    dropped: u64,
    coalesced: u64,
}

impl HidState {
    fn new(kind: HidKind) -> Self {
        Self {
            kind,
            address: 0,
            configured: false,
            halted: false,
            active: false,
            idle: 0,
            protocol: PROTOCOL_REPORT,
            leds: 0,
            modifiers: 0,
            keys: Vec::new(),
            buttons: 0,
            x: 0,
            y: 0,
            wheel: 0,
            queue: VecDeque::new(),
            dropped: 0,
            coalesced: 0,
        }
    }

    /// Back to the Default state: address 0, not configured, no halt, idle 0, Report protocol,
    /// no queued reports. Keys, buttons and coordinates are kept (a bus reset does not touch
    /// them); HCRST also releases them ([`Self::host_controller_reset`]).
    fn reset(&mut self) {
        self.address = 0;
        self.configured = false;
        self.halted = false;
        self.idle = 0;
        self.protocol = PROTOCOL_REPORT;
        self.leds = 0;
        self.wheel = 0;
        self.queue.clear();
    }

    fn host_controller_reset(&mut self) {
        self.reset();
        self.modifiers = 0;
        self.keys.clear();
        self.buttons = 0;
    }

    /// Reports exist only while the device is configured and its endpoint is running.
    fn reporting(&self) -> bool {
        self.configured && self.active
    }

    // ---------------------------------------------------------------- control requests

    fn control(&mut self, setup: &[u8; 8], out_data: &[u8]) -> ControlResponse {
        let request_type = setup[0];
        let request = setup[1];
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let index = u16::from_le_bytes([setup[4], setup[5]]);
        let length = u16::from_le_bytes([setup[6], setup[7]]);
        let data = |bytes: Vec<u8>| ControlResponse::Data(bytes);
        let stall = ControlResponse::Stall;
        // A request without a data stage must say so.
        let no_data = length == 0;
        match (request_type, request) {
            // GET_STATUS: device, interface (configured only), endpoint.
            (0x80, 0) if value == 0 && index == 0 => data(vec![0, 0]),
            (0x81, 0) if self.configured && value == 0 && index == 0 => data(vec![0, 0]),
            (0x82, 0) if value == 0 && matches!(index, 0x00 | 0x80) => data(vec![0, 0]),
            (0x82, 0) if value == 0 && index == 0x81 && self.configured => {
                data(vec![u8::from(self.halted), 0])
            }
            // CLEAR_FEATURE / SET_FEATURE(ENDPOINT_HALT) of endpoint 0x81, configured only.
            (0x02, 1 | 3) if value == 0 && index == 0x81 && no_data && self.configured => {
                self.halted = request == 3;
                ControlResponse::Ack
            }
            (0x80, 6) => self.get_descriptor(value, index),
            (0x81, 6) => self.get_class_descriptor(value, index),
            (0x80, 8) if value == 0 && index == 0 => data(vec![u8::from(self.configured)]),
            (0x00, 9) if index == 0 && no_data => self.set_configuration(value),
            (0x81, 10) if self.configured && value == 0 && index == 0 => data(vec![0]),
            (0x01, 11) if self.configured && value == 0 && index == 0 && no_data => {
                self.halted = false;
                self.queue.clear();
                ControlResponse::Ack
            }
            // HID class requests, interface 0.
            (0xA1, 0x01) => self.get_report(value, index),
            (0x21, 0x09) => self.set_report(value, index, length, out_data),
            (0xA1, 0x02) if value == 0 && index == 0 => data(vec![self.idle]),
            (0x21, 0x0A) if value & 0xFF == 0 && index == 0 && no_data => {
                self.idle = (value >> 8) as u8;
                ControlResponse::Ack
            }
            (0xA1, 0x03) if self.kind == HidKind::Keyboard && value == 0 && index == 0 => {
                data(vec![self.protocol])
            }
            (0x21, 0x0B)
                if self.kind == HidKind::Keyboard && index == 0 && no_data && value <= 1 =>
            {
                self.protocol = if value == 0 {
                    PROTOCOL_BOOT
                } else {
                    PROTOCOL_REPORT
                };
                ControlResponse::Ack
            }
            // Everything else, including SET_ADDRESS (the controller's job), SET_DESCRIPTOR,
            // SYNCH_FRAME, features of the device or the interface, and unknown requests.
            _ => stall,
        }
    }

    /// GET_DESCRIPTOR addressed to the device: device, configuration 0, strings.
    fn get_descriptor(&self, value: u16, index: u16) -> ControlResponse {
        let kind_of_descriptor = (value >> 8) as u8;
        let descriptor_index = (value & 0xFF) as u8;
        let bytes = match kind_of_descriptor {
            1 if descriptor_index == 0 && index == 0 => Some(device_descriptor(self.kind)),
            2 if descriptor_index == 0 && index == 0 => Some(configuration_descriptor(self.kind)),
            // Index 0 is the language list, 1 and 2 the strings, any other index (0xEE, the
            // Microsoft OS string, too) is a Request Error.
            3 => string_descriptor(self.kind, descriptor_index),
            // Device_Qualifier (6), Other_Speed_Configuration (7), BOS (15), Physical and the
            // rest: Request Error.
            _ => None,
        };
        bytes.map_or(ControlResponse::Stall, ControlResponse::Data)
    }

    /// GET_DESCRIPTOR addressed to the interface: the HID descriptor (0x21) and the Report
    /// descriptor (0x22) of interface 0.
    fn get_class_descriptor(&self, value: u16, index: u16) -> ControlResponse {
        let descriptor_index = value & 0xFF;
        if index != 0 || descriptor_index != 0 {
            return ControlResponse::Stall;
        }
        match value >> 8 {
            0x21 => ControlResponse::Data(hid_descriptor(self.kind)),
            0x22 => ControlResponse::Data(report_descriptor(self.kind)),
            _ => ControlResponse::Stall,
        }
    }

    /// SET_CONFIGURATION: 0 or 1, in the Address state or later (not in the Default state). It
    /// resets the halt of the endpoint and drops queued reports.
    fn set_configuration(&mut self, value: u16) -> ControlResponse {
        if self.address == 0 || value > 1 {
            return ControlResponse::Stall;
        }
        self.configured = value == 1;
        self.halted = false;
        self.queue.clear();
        self.wheel = 0;
        ControlResponse::Ack
    }

    /// GET_REPORT: the current Input report (the pointer's wheel byte is 0, nothing is consumed),
    /// or the keyboard's LED byte.
    fn get_report(&self, value: u16, index: u16) -> ControlResponse {
        let report_type = (value >> 8) as u8;
        if value & 0xFF != 0 || index != 0 {
            return ControlResponse::Stall;
        }
        match (self.kind, report_type) {
            (_, REPORT_TYPE_INPUT) => ControlResponse::Data(self.current_report()),
            (HidKind::Keyboard, REPORT_TYPE_OUTPUT) => ControlResponse::Data(vec![self.leds]),
            _ => ControlResponse::Stall,
        }
    }

    /// SET_REPORT: the keyboard's one-byte LED Output report.
    fn set_report(
        &mut self,
        value: u16,
        index: u16,
        length: u16,
        out_data: &[u8],
    ) -> ControlResponse {
        let report_type = (value >> 8) as u8;
        if self.kind != HidKind::Keyboard
            || report_type != REPORT_TYPE_OUTPUT
            || value & 0xFF != 0
            || index != 0
            || length != 1
            || out_data.len() != 1
        {
            return ControlResponse::Stall;
        }
        self.leds = out_data[0] & LED_MASK;
        ControlResponse::Ack
    }

    // ---------------------------------------------------------------- reports

    /// The Input report for the current state, without the wheel.
    fn current_report(&self) -> Vec<u8> {
        match self.kind {
            HidKind::Keyboard => {
                let mut report = vec![self.modifiers, 0, 0, 0, 0, 0, 0, 0];
                if self.keys.len() > 6 {
                    report[2..].fill(ERROR_ROLL_OVER);
                } else {
                    report[2..2 + self.keys.len()].copy_from_slice(&self.keys);
                }
                report
            }
            HidKind::Pointer => self.pointer_report(0),
        }
    }

    fn pointer_report(&self, wheel: i8) -> Vec<u8> {
        let mut report = vec![self.buttons];
        report.extend_from_slice(&self.x.to_le_bytes());
        report.extend_from_slice(&self.y.to_le_bytes());
        report.push(wheel as u8);
        report
    }

    /// Queues the report of a state change, if the device reports at all. The pointer report
    /// carries the wheel movement since the previous report.
    fn emit(&mut self) {
        if !self.reporting() {
            return;
        }
        let report = match self.kind {
            HidKind::Keyboard => self.current_report(),
            HidKind::Pointer => {
                let sent = self.wheel.clamp(-127, 127);
                self.wheel -= sent;
                self.pointer_report(sent as i8)
            }
        };
        self.push_report(report);
    }

    /// A full queue replaces its last report: reports are snapshots, only intermediate states
    /// are lost. A replaced pointer report hands its wheel movement to the new one.
    fn push_report(&mut self, mut report: Vec<u8>) {
        if self.queue.len() < REPORT_QUEUE_LIMIT {
            self.queue.push_back(report);
            return;
        }
        self.coalesced += 1;
        if let (HidKind::Pointer, Some(old)) = (self.kind, self.queue.back()) {
            let sum = i32::from(old[5] as i8) + i32::from(report[5] as i8);
            let sent = sum.clamp(-127, 127);
            self.wheel += sum - sent;
            report[5] = sent as i8 as u8;
        }
        if let Some(last) = self.queue.back_mut() {
            *last = report;
        }
    }

    // ---------------------------------------------------------------- input events

    /// The event is one this device reacts to (keys for the keyboard, the rest for the pointer).
    fn owns(&self, event: &InputEvent) -> bool {
        matches!(event, InputEvent::Key { .. }) == (self.kind == HidKind::Keyboard)
    }

    fn input(&mut self, event: &InputEvent) {
        match (self.kind, *event) {
            (HidKind::Keyboard, InputEvent::Key { usage, pressed }) => self.key(usage, pressed),
            (HidKind::Pointer, InputEvent::PointerAbs { x, y }) => self.pointer_abs(x, y),
            (HidKind::Pointer, InputEvent::Buttons { mask }) => self.buttons(mask),
            (HidKind::Pointer, InputEvent::Wheel { delta }) => self.wheel_event(delta),
            _ => {}
        }
    }

    /// A key or modifier by USB HID Usage ID (page 0x07): keys 0x04..=0xDD, modifiers
    /// 0xE0..=0xE7; any other Usage ID is dropped and counted.
    fn key(&mut self, usage: u8, pressed: bool) {
        let changed = match usage {
            0xE0..=0xE7 => {
                let bit = 1 << (usage - 0xE0);
                let was = self.modifiers & bit != 0;
                if pressed {
                    self.modifiers |= bit;
                } else {
                    self.modifiers &= !bit;
                }
                was != pressed
            }
            0x04..=0xDD => match (self.keys.iter().position(|key| *key == usage), pressed) {
                (None, true) => {
                    self.keys.push(usage);
                    true
                }
                (Some(position), false) => {
                    self.keys.remove(position);
                    true
                }
                _ => false,
            },
            _ => {
                self.dropped += 1;
                false
            }
        };
        if changed {
            self.emit();
        }
    }

    /// Absolute coordinates 0..=32767; anything else is dropped and counted.
    fn pointer_abs(&mut self, x: u16, y: u16) {
        if x > POINTER_MAX || y > POINTER_MAX {
            self.dropped += 1;
        } else if (x, y) != (self.x, self.y) {
            (self.x, self.y) = (x, y);
            self.emit();
        }
    }

    /// Button mask, bits 3 and up ignored.
    fn buttons(&mut self, mask: u8) {
        let mask = mask & 0b111;
        if mask != self.buttons {
            self.buttons = mask;
            self.emit();
        }
    }

    /// Wheel movement in -127..=127 (-128 is out of range: dropped and counted), accumulated
    /// until a report carries it.
    fn wheel_event(&mut self, delta: i8) {
        if delta == i8::MIN {
            self.dropped += 1;
        } else if delta != 0 && self.reporting() {
            // Without a reader the movement is not kept: it would come back as a phantom scroll.
            self.wheel = self.wheel.saturating_add(i32::from(delta));
            self.emit();
        }
    }
}

impl UsbDevice for HidDevice {
    fn address(&self) -> u8 {
        self.with(|state| state.address)
    }

    fn set_address(&mut self, address: u8) {
        self.with(|state| state.address = address);
    }

    fn reset(&mut self) {
        self.with(HidState::reset);
    }

    fn host_controller_reset(&mut self) {
        self.with(HidState::host_controller_reset);
    }

    fn control(&mut self, setup: &[u8; 8], out_data: &[u8]) -> ControlResponse {
        self.with(|state| state.control(setup, out_data))
    }

    fn interrupt_in(&mut self) -> InterruptStatus {
        self.with(|state| {
            if state.halted {
                InterruptStatus::Stall
            } else {
                state.queue.front().map_or(InterruptStatus::Nak, |report| {
                    InterruptStatus::Report(report.len())
                })
            }
        })
    }

    fn take_interrupt_report(&mut self) -> Vec<u8> {
        self.with(|state| state.queue.pop_front().unwrap_or_default())
    }

    fn input(&mut self, event: &InputEvent) {
        self.with(|state| state.input(event));
    }

    fn input_ready(&self, event: &InputEvent) -> bool {
        self.with(|state| {
            !(state.reporting() && state.owns(event) && state.queue.len() >= REPORT_QUEUE_LIMIT)
        })
    }

    fn set_interrupt_endpoint_active(&mut self, active: bool) {
        self.with(|state| {
            state.active = active;
            if !active {
                state.queue.clear();
                state.wheel = 0;
            }
        });
    }
}
