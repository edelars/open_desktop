//! USB standard and HID class requests, and the descriptor walk that finds a boot keyboard
//! (USB 2.0 chapter 9, HID 1.11 section 7; XHCI_DEVICE.md section 10).
//!
//! Pure logic, covered by host tests.

pub(super) const DEVICE_DESCRIPTOR_BYTES: usize = 18;
pub(super) const CONFIGURATION_HEADER_BYTES: usize = 9;
/// Largest configuration descriptor set the driver reads (the board keyboard's is 34 bytes).
pub(super) const MAX_CONFIGURATION_BYTES: usize = 256;

const DESCRIPTOR_DEVICE: u8 = 1;
const DESCRIPTOR_CONFIGURATION: u8 = 2;
const DESCRIPTOR_INTERFACE: u8 = 4;
const DESCRIPTOR_ENDPOINT: u8 = 5;

const CLASS_HID: u8 = 3;
const SUBCLASS_BOOT: u8 = 1;
const PROTOCOL_KEYBOARD: u8 = 1;

/// A USB SETUP packet (USB 2.0 table 9-2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SetupPacket {
    pub(super) request_type: u8,
    pub(super) request: u8,
    pub(super) value: u16,
    pub(super) index: u16,
    pub(super) length: u16,
}

impl SetupPacket {
    /// The eight packet bytes as the little-endian parameter of a Setup Stage TRB.
    pub(super) const fn as_u64(self) -> u64 {
        u64::from_le_bytes([
            self.request_type,
            self.request,
            self.value as u8,
            (self.value >> 8) as u8,
            self.index as u8,
            (self.index >> 8) as u8,
            self.length as u8,
            (self.length >> 8) as u8,
        ])
    }

    pub(super) const fn get_device_descriptor() -> Self {
        Self::get_descriptor(DESCRIPTOR_DEVICE, DEVICE_DESCRIPTOR_BYTES as u16)
    }

    pub(super) const fn get_configuration_descriptor(length: u16) -> Self {
        Self::get_descriptor(DESCRIPTOR_CONFIGURATION, length)
    }

    const fn get_descriptor(kind: u8, length: u16) -> Self {
        Self {
            request_type: 0x80,
            request: 6,
            value: (kind as u16) << 8,
            index: 0,
            length,
        }
    }

    pub(super) const fn set_configuration(value: u8) -> Self {
        Self {
            request_type: 0x00,
            request: 9,
            value: value as u16,
            index: 0,
            length: 0,
        }
    }

    /// HID SET_PROTOCOL; 0 selects the boot protocol (HID 1.11 section 7.2.6).
    pub(super) const fn set_protocol(interface: u8, protocol: u16) -> Self {
        Self {
            request_type: 0x21,
            request: 0x0b,
            value: protocol,
            index: interface as u16,
            length: 0,
        }
    }

    /// HID SET_IDLE; duration 0 means "report only on change" (section 7.2.4).
    pub(super) const fn set_idle(interface: u8, duration: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 0x0a,
            value: (duration as u16) << 8,
            index: interface as u16,
            length: 0,
        }
    }

    /// HID SET_REPORT of the one-byte Output report (keyboard LEDs).
    pub(super) const fn set_output_report(interface: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 0x09,
            value: 2 << 8,
            index: interface as u16,
            length: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DescriptorError {
    Truncated,
    BadType,
    BadLength,
}

/// Fields of the device descriptor the driver uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DeviceDescriptor {
    pub(super) max_packet_size0: u8,
    pub(super) vendor: u16,
    pub(super) product: u16,
    pub(super) configurations: u8,
}

pub(super) fn parse_device(bytes: &[u8]) -> Result<DeviceDescriptor, DescriptorError> {
    if bytes.len() < DEVICE_DESCRIPTOR_BYTES {
        return Err(DescriptorError::Truncated);
    }
    if bytes[0] as usize != DEVICE_DESCRIPTOR_BYTES || bytes[1] != DESCRIPTOR_DEVICE {
        return Err(DescriptorError::BadType);
    }
    Ok(DeviceDescriptor {
        max_packet_size0: bytes[7],
        vendor: u16::from_le_bytes([bytes[8], bytes[9]]),
        product: u16::from_le_bytes([bytes[10], bytes[11]]),
        configurations: bytes[17],
    })
}

/// wTotalLength of a configuration descriptor, from its first 9 bytes.
pub(super) fn configuration_total_length(header: &[u8]) -> Result<usize, DescriptorError> {
    if header.len() < CONFIGURATION_HEADER_BYTES {
        return Err(DescriptorError::Truncated);
    }
    if header[0] as usize != CONFIGURATION_HEADER_BYTES || header[1] != DESCRIPTOR_CONFIGURATION {
        return Err(DescriptorError::BadType);
    }
    let total = usize::from(u16::from_le_bytes([header[2], header[3]]));
    if total < CONFIGURATION_HEADER_BYTES {
        return Err(DescriptorError::BadLength);
    }
    Ok(total)
}

/// A HID boot-protocol keyboard interface and its Interrupt IN endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct KeyboardInterface {
    pub(super) configuration: u8,
    pub(super) interface: u8,
    pub(super) endpoint_address: u8,
    pub(super) max_packet_size: u16,
    pub(super) interval: u8,
}

/// Walks a complete configuration descriptor set and returns the first interface with class
/// HID (3), subclass boot (1), protocol keyboard (1) together with its Interrupt IN endpoint.
/// `Ok(None)`: a valid configuration without such an interface (for example the pointer).
pub(super) fn parse_keyboard(bytes: &[u8]) -> Result<Option<KeyboardInterface>, DescriptorError> {
    let total = configuration_total_length(bytes)?;
    if bytes.len() < total {
        return Err(DescriptorError::Truncated);
    }
    let bytes = &bytes[..total];
    let configuration = bytes[5];
    let mut offset = CONFIGURATION_HEADER_BYTES;
    let mut interface = None;
    while offset < total {
        if total - offset < 2 {
            return Err(DescriptorError::Truncated);
        }
        let length = usize::from(bytes[offset]);
        let kind = bytes[offset + 1];
        if length < 2 {
            return Err(DescriptorError::BadLength);
        }
        if length > total - offset {
            return Err(DescriptorError::Truncated);
        }
        let descriptor = &bytes[offset..offset + length];
        match kind {
            DESCRIPTOR_INTERFACE => {
                if length < 9 {
                    return Err(DescriptorError::BadLength);
                }
                interface = (descriptor[5] == CLASS_HID
                    && descriptor[6] == SUBCLASS_BOOT
                    && descriptor[7] == PROTOCOL_KEYBOARD)
                    .then_some(descriptor[2]);
            }
            DESCRIPTOR_ENDPOINT => {
                if length < 7 {
                    return Err(DescriptorError::BadLength);
                }
                let address = descriptor[2];
                // Interrupt (transfer type 3), IN (bit 7).
                if let Some(interface) = interface
                    && address & 0x80 != 0
                    && descriptor[3] & 3 == 3
                {
                    return Ok(Some(KeyboardInterface {
                        configuration,
                        interface,
                        endpoint_address: address,
                        max_packet_size: u16::from_le_bytes([descriptor[4], descriptor[5]]),
                        interval: descriptor[6],
                    }));
                }
            }
            _ => {}
        }
        offset += length;
    }
    Ok(None)
}

/// xHCI Endpoint Context Interval (exponent of 125 us microframes) for a Full-speed interrupt
/// endpoint with `b_interval` frames: rounded down, clamped to 3..=10 (xHCI 1.2 table 6-12;
/// 10 ms -> 80 microframes -> 6).
pub(super) fn full_speed_interval_exponent(b_interval: u8) -> u32 {
    // A loop, not `leading_zeros`: the firmware CPU profile has no BSR/LZCNT.
    let microframes = u32::from(b_interval.max(1)) * 8;
    let mut exponent = 3;
    while exponent < 10 && microframes >= 1 << (exponent + 1) {
        exponent += 1;
    }
    exponent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The board keyboard's set: Configuration, Interface (HID boot keyboard), HID, Endpoint.
    fn keyboard_configuration() -> [u8; 34] {
        [
            9, 2, 34, 0, 1, 1, 0, 0x80, 50, // configuration, wTotalLength 34
            9, 4, 0, 0, 1, 3, 1, 1, 0, // interface 0: HID, boot, keyboard
            9, 0x21, 0x11, 0x01, 0, 1, 0x22, 64, 0, // HID descriptor
            7, 5, 0x81, 3, 8, 0, 10, // endpoint 0x81 interrupt IN, 8 bytes, 10 ms
        ]
    }

    #[test]
    fn setup_packets_are_encoded_little_endian() {
        assert_eq!(
            SetupPacket::get_device_descriptor().as_u64(),
            0x0012_0000_0100_0680
        );
        assert_eq!(
            SetupPacket::get_configuration_descriptor(34).as_u64(),
            0x0022_0000_0200_0680
        );
        assert_eq!(
            SetupPacket::set_configuration(1).as_u64(),
            0x0000_0000_0001_0900
        );
        assert_eq!(
            SetupPacket::set_protocol(0, 0).as_u64(),
            0x0000_0000_0000_0B21
        );
        assert_eq!(SetupPacket::set_idle(2, 0).as_u64(), 0x0000_0002_0000_0A21);
        assert_eq!(
            SetupPacket::set_output_report(0).as_u64(),
            0x0001_0000_0200_0921
        );
    }

    #[test]
    fn device_descriptor_fields_are_extracted() {
        let mut bytes = [0u8; 18];
        bytes[0] = 18;
        bytes[1] = 1;
        bytes[7] = 64;
        bytes[8..10].copy_from_slice(&0xD1A6u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&0x0101u16.to_le_bytes());
        bytes[17] = 1;
        assert_eq!(
            parse_device(&bytes),
            Ok(DeviceDescriptor {
                max_packet_size0: 64,
                vendor: 0xD1A6,
                product: 0x0101,
                configurations: 1
            })
        );
        assert_eq!(parse_device(&bytes[..17]), Err(DescriptorError::Truncated));
        bytes[1] = 2;
        assert_eq!(parse_device(&bytes), Err(DescriptorError::BadType));
    }

    #[test]
    fn configuration_header_gives_the_total_length() {
        let set = keyboard_configuration();
        assert_eq!(configuration_total_length(&set[..9]), Ok(34));
        assert_eq!(
            configuration_total_length(&set[..8]),
            Err(DescriptorError::Truncated)
        );
        let mut bad = set;
        bad[2] = 5;
        assert_eq!(
            configuration_total_length(&bad[..9]),
            Err(DescriptorError::BadLength)
        );
        bad[1] = 1;
        assert_eq!(
            configuration_total_length(&bad[..9]),
            Err(DescriptorError::BadType)
        );
    }

    #[test]
    fn boot_keyboard_interface_and_endpoint_are_found() {
        assert_eq!(
            parse_keyboard(&keyboard_configuration()),
            Ok(Some(KeyboardInterface {
                configuration: 1,
                interface: 0,
                endpoint_address: 0x81,
                max_packet_size: 8,
                interval: 10,
            }))
        );
    }

    #[test]
    fn pointer_configuration_has_no_keyboard() {
        let mut set = keyboard_configuration();
        set[15] = 0; // subclass 0
        set[16] = 0; // protocol 0
        set[31] = 6; // wMaxPacketSize 6 as the pointer reports
        assert_eq!(parse_keyboard(&set), Ok(None));
        // A keyboard-class interface whose endpoint is OUT does not count.
        let mut out_only = keyboard_configuration();
        out_only[29] = 0x01;
        assert_eq!(parse_keyboard(&out_only), Ok(None));
    }

    #[test]
    fn malformed_descriptor_sets_are_rejected_not_skipped() {
        let set = keyboard_configuration();
        assert_eq!(parse_keyboard(&set[..33]), Err(DescriptorError::Truncated));
        let mut zero_length = set;
        zero_length[18] = 0;
        assert_eq!(
            parse_keyboard(&zero_length),
            Err(DescriptorError::BadLength)
        );
        let mut overrun = set;
        overrun[27] = 40;
        assert_eq!(parse_keyboard(&overrun), Err(DescriptorError::Truncated));
        // A 6-byte endpoint descriptor is too short (needs 7).
        let mut short_endpoint = set;
        short_endpoint[27] = 6;
        assert_eq!(
            parse_keyboard(&short_endpoint),
            Err(DescriptorError::BadLength)
        );
    }

    #[test]
    fn interval_exponent_matches_the_contract() {
        // bInterval 10 ms = 80 microframes, rounded down to 2^6 (XHCI_DEVICE.md section 8).
        assert_eq!(full_speed_interval_exponent(10), 6);
        assert_eq!(full_speed_interval_exponent(1), 3);
        assert_eq!(full_speed_interval_exponent(0), 3);
        assert_eq!(full_speed_interval_exponent(255), 10);
        assert_eq!(full_speed_interval_exponent(8), 6);
        assert_eq!(full_speed_interval_exponent(16), 7);
    }
}
