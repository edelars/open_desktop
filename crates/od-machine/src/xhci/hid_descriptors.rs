//! USB and HID descriptors of the two board devices (`docs/specs/XHCI_DEVICE.md` section 10):
//! the standard descriptors of USB 2.0 chapter 9, and the report descriptors, which are encoded
//! from a table of HID items (HID 1.11 section 6.2.2) by [`ReportDescriptor`], never pasted as
//! byte arrays.

/// Which of the two devices a descriptor describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HidKind {
    /// Port 1: a boot-protocol keyboard.
    Keyboard,
    /// Port 2: an absolute pointer (Mouse application collection, X and Y 0..32767).
    Pointer,
}

pub(super) const VENDOR_ID: u16 = 0xD1A6;
pub(super) const CONFIGURATION_TOTAL_LENGTH: usize = 34;

impl HidKind {
    pub(super) fn product_id(self) -> u16 {
        match self {
            Self::Keyboard => 0x0101,
            Self::Pointer => 0x0102,
        }
    }

    /// Length of the Input report in bytes, also wMaxPacketSize of the interrupt endpoint.
    pub(super) fn report_length(self) -> usize {
        match self {
            Self::Keyboard => 8,
            Self::Pointer => 6,
        }
    }

    fn product_string(self) -> &'static str {
        match self {
            Self::Keyboard => "Virtual HID Keyboard",
            Self::Pointer => "Virtual HID Absolute Pointer",
        }
    }

    /// bInterfaceSubClass (1 = boot interface) and bInterfaceProtocol (1 = keyboard).
    fn interface_subclass_and_protocol(self) -> (u8, u8) {
        match self {
            Self::Keyboard => (1, 1),
            Self::Pointer => (0, 0),
        }
    }
}

/// Device descriptor (USB 2.0 table 9-8): 18 bytes.
pub(super) fn device_descriptor(kind: HidKind) -> Vec<u8> {
    let mut bytes = vec![18, 1];
    bytes.extend_from_slice(&0x0200u16.to_le_bytes()); // bcdUSB
    bytes.extend_from_slice(&[0, 0, 0]); // class, subclass, protocol: in the interface
    bytes.push(64); // bMaxPacketSize0
    bytes.extend_from_slice(&VENDOR_ID.to_le_bytes());
    bytes.extend_from_slice(&kind.product_id().to_le_bytes());
    bytes.extend_from_slice(&0x0100u16.to_le_bytes()); // bcdDevice
    bytes.extend_from_slice(&[1, 2, 0, 1]); // iManufacturer, iProduct, iSerialNumber, configurations
    bytes
}

/// The HID class descriptor (HID 1.11 section 6.2.1): 9 bytes, one Report descriptor.
pub(super) fn hid_descriptor(kind: HidKind) -> Vec<u8> {
    let mut bytes = vec![9, 0x21];
    bytes.extend_from_slice(&0x0111u16.to_le_bytes()); // bcdHID
    bytes.extend_from_slice(&[0, 1, 0x22]); // country, descriptors, type Report
    bytes.extend_from_slice(&(report_descriptor(kind).len() as u16).to_le_bytes());
    bytes
}

/// The configuration set: Configuration, Interface, HID and Endpoint descriptors, 34 bytes.
pub(super) fn configuration_descriptor(kind: HidKind) -> Vec<u8> {
    let mut bytes = vec![9, 2];
    bytes.extend_from_slice(&(CONFIGURATION_TOTAL_LENGTH as u16).to_le_bytes());
    bytes.extend_from_slice(&[1, 1, 0, 0x80, 50]); // interfaces, value, string, attributes, 100 mA
    let (subclass, protocol) = kind.interface_subclass_and_protocol();
    bytes.extend_from_slice(&[9, 4, 0, 0, 1, 3, subclass, protocol, 0]);
    bytes.extend_from_slice(&hid_descriptor(kind));
    bytes.extend_from_slice(&[7, 5, 0x81, 0x03]);
    bytes.extend_from_slice(&(kind.report_length() as u16).to_le_bytes());
    bytes.push(10); // bInterval
    debug_assert_eq!(bytes.len(), CONFIGURATION_TOTAL_LENGTH);
    bytes
}

/// String descriptor `index` (USB 2.0 section 9.6.7): 0 is the LANGID list, 1 the manufacturer,
/// 2 the product; every other index does not exist.
pub(super) fn string_descriptor(kind: HidKind, index: u8) -> Option<Vec<u8>> {
    let text = match index {
        0 => return Some(vec![4, 3, 0x09, 0x04]),
        1 => "Open Desktop",
        2 => kind.product_string(),
        _ => return None,
    };
    let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut bytes = vec![(2 + units.len()) as u8, 3];
    bytes.extend_from_slice(&units);
    Some(bytes)
}

/// Item kinds of HID short items (HID 1.11 section 6.2.2.2).
#[derive(Clone, Copy)]
enum ItemKind {
    Main = 0,
    Global = 1,
    Local = 2,
}

/// A report descriptor under construction: each call appends one short item.
pub(super) struct ReportDescriptor(Vec<u8>);

impl ReportDescriptor {
    pub(super) fn new() -> Self {
        Self(Vec::new())
    }

    /// One short item: prefix (tag, type, size) and the minimal little-endian data. Signed
    /// items (Logical Minimum and Maximum) use the shortest two's complement form that keeps
    /// the sign, so 0xE7 needs two bytes.
    fn item(mut self, kind: ItemKind, tag: u8, value: i32, signed: bool) -> Self {
        let size: usize = if signed {
            if i8::try_from(value).is_ok() {
                1
            } else if i16::try_from(value).is_ok() {
                2
            } else {
                4
            }
        } else if value <= 0xFF {
            1
        } else if value <= 0xFFFF {
            2
        } else {
            4
        };
        let size_code = match size {
            1 => 1,
            2 => 2,
            _ => 3,
        };
        self.0.push((tag << 4) | ((kind as u8) << 2) | size_code);
        self.0.extend_from_slice(&value.to_le_bytes()[..size]);
        self
    }

    pub(super) fn usage_page(self, page: u16) -> Self {
        self.item(ItemKind::Global, 0x0, i32::from(page), false)
    }

    pub(super) fn logical_minimum(self, value: i32) -> Self {
        self.item(ItemKind::Global, 0x1, value, true)
    }

    pub(super) fn logical_maximum(self, value: i32) -> Self {
        self.item(ItemKind::Global, 0x2, value, true)
    }

    pub(super) fn report_size(self, bits: u32) -> Self {
        self.item(ItemKind::Global, 0x7, bits as i32, false)
    }

    pub(super) fn report_count(self, count: u32) -> Self {
        self.item(ItemKind::Global, 0x9, count as i32, false)
    }

    pub(super) fn usage(self, usage: u16) -> Self {
        self.item(ItemKind::Local, 0x0, i32::from(usage), false)
    }

    pub(super) fn usage_minimum(self, usage: u16) -> Self {
        self.item(ItemKind::Local, 0x1, i32::from(usage), false)
    }

    pub(super) fn usage_maximum(self, usage: u16) -> Self {
        self.item(ItemKind::Local, 0x2, i32::from(usage), false)
    }

    pub(super) fn input(self, flags: u8) -> Self {
        self.item(ItemKind::Main, 0x8, i32::from(flags), false)
    }

    pub(super) fn output(self, flags: u8) -> Self {
        self.item(ItemKind::Main, 0x9, i32::from(flags), false)
    }

    pub(super) fn collection(self, kind: u8) -> Self {
        self.item(ItemKind::Main, 0xA, i32::from(kind), false)
    }

    /// End Collection has no data (size 0).
    pub(super) fn end_collection(mut self) -> Self {
        self.0.push(0xC << 4);
        self
    }

    pub(super) fn build(self) -> Vec<u8> {
        self.0
    }
}

/// Main item flags (HID 1.11 section 6.2.2.5).
const DATA_VARIABLE_ABSOLUTE: u8 = 0x02;
const CONSTANT: u8 = 0x01;
const DATA_ARRAY_ABSOLUTE: u8 = 0x00;
const DATA_VARIABLE_RELATIVE: u8 = 0x06;
/// Collection types.
const COLLECTION_PHYSICAL: u8 = 0x00;
const COLLECTION_APPLICATION: u8 = 0x01;

const PAGE_GENERIC_DESKTOP: u16 = 0x01;
const PAGE_KEYBOARD: u16 = 0x07;
const PAGE_LEDS: u16 = 0x08;
const PAGE_BUTTON: u16 = 0x09;

/// The report descriptor of a device, built from its element table (contract sections 10.3 and
/// 10.4): 64 bytes for the keyboard, 63 for the pointer.
pub(super) fn report_descriptor(kind: HidKind) -> Vec<u8> {
    match kind {
        HidKind::Keyboard => keyboard_report_descriptor(),
        HidKind::Pointer => pointer_report_descriptor(),
    }
}

/// Boot-compatible keyboard (HID 1.11 appendix B.1): modifiers, a reserved byte, five LEDs with
/// padding, six key codes.
fn keyboard_report_descriptor() -> Vec<u8> {
    ReportDescriptor::new()
        .usage_page(PAGE_GENERIC_DESKTOP)
        .usage(0x06) // Keyboard
        .collection(COLLECTION_APPLICATION)
        .usage_page(PAGE_KEYBOARD)
        .usage_minimum(0xE0)
        .usage_maximum(0xE7)
        .logical_minimum(0)
        .logical_maximum(1)
        .report_size(1)
        .report_count(8)
        .input(DATA_VARIABLE_ABSOLUTE) // modifier keys
        .report_count(1)
        .report_size(8)
        .input(CONSTANT) // reserved byte
        .report_count(5)
        .report_size(1)
        .usage_page(PAGE_LEDS)
        .usage_minimum(1)
        .usage_maximum(5)
        .output(DATA_VARIABLE_ABSOLUTE) // LEDs
        .report_count(1)
        .report_size(3)
        .output(CONSTANT) // LED padding
        .report_count(6)
        .report_size(8)
        .logical_minimum(0)
        .logical_maximum(0xE7)
        .usage_page(PAGE_KEYBOARD)
        .usage_minimum(0)
        .usage_maximum(0xE7)
        .input(DATA_ARRAY_ABSOLUTE) // key codes
        .end_collection()
        .build()
}

/// Mouse application collection with a Pointer physical collection: three buttons, absolute X
/// and Y of 16 bits (0..32767), a relative wheel.
fn pointer_report_descriptor() -> Vec<u8> {
    ReportDescriptor::new()
        .usage_page(PAGE_GENERIC_DESKTOP)
        .usage(0x02) // Mouse
        .collection(COLLECTION_APPLICATION)
        .usage(0x01) // Pointer
        .collection(COLLECTION_PHYSICAL)
        .usage_page(PAGE_BUTTON)
        .usage_minimum(1)
        .usage_maximum(3)
        .logical_minimum(0)
        .logical_maximum(1)
        .report_size(1)
        .report_count(3)
        .input(DATA_VARIABLE_ABSOLUTE) // buttons
        .report_count(1)
        .report_size(5)
        .input(CONSTANT) // padding
        .usage_page(PAGE_GENERIC_DESKTOP)
        .usage(0x30) // X
        .usage(0x31) // Y
        .logical_minimum(0)
        .logical_maximum(32767)
        .report_size(16)
        .report_count(2)
        .input(DATA_VARIABLE_ABSOLUTE)
        .usage(0x38) // Wheel
        .logical_minimum(-127)
        .logical_maximum(127)
        .report_size(8)
        .report_count(1)
        .input(DATA_VARIABLE_RELATIVE)
        .end_collection()
        .end_collection()
        .build()
}
