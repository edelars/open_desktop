//! xHCI TRB layouts, ring cursors and context words (XHCI_DEVICE.md sections 8 and 9).
//!
//! Pure logic: no MMIO and no allocator, so it is covered by host tests.

pub(super) const TRB_BYTES: u64 = 16;

pub(super) mod trb_type {
    pub(in crate::usb) const NORMAL: u32 = 1;
    pub(in crate::usb) const SETUP_STAGE: u32 = 2;
    pub(in crate::usb) const DATA_STAGE: u32 = 3;
    pub(in crate::usb) const STATUS_STAGE: u32 = 4;
    pub(in crate::usb) const LINK: u32 = 6;
    pub(in crate::usb) const ENABLE_SLOT: u32 = 9;
    pub(in crate::usb) const DISABLE_SLOT: u32 = 10;
    pub(in crate::usb) const ADDRESS_DEVICE: u32 = 11;
    pub(in crate::usb) const CONFIGURE_ENDPOINT: u32 = 12;
    pub(in crate::usb) const TRANSFER_EVENT: u32 = 32;
    pub(in crate::usb) const COMMAND_COMPLETION: u32 = 33;
}

/// Completion codes the driver distinguishes (xHCI 1.2, table 6-90).
pub(super) mod completion {
    pub(in crate::usb) const SUCCESS: u32 = 1;
    pub(in crate::usb) const SHORT_PACKET: u32 = 13;
}

const CYCLE: u32 = 1;
const TOGGLE_CYCLE: u32 = 1 << 1;
const INTERRUPT_ON_SHORT_PACKET: u32 = 1 << 2;
const INTERRUPT_ON_COMPLETION: u32 = 1 << 5;
const IMMEDIATE_DATA: u32 = 1 << 6;
const DIRECTION_IN: u32 = 1 << 16;
const BLOCK_SET_ADDRESS: u32 = 1 << 9;

/// Control transfer type field (Setup Stage TRB, bits 17:16).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TransferType {
    NoData = 0,
    Out = 2,
    In = 3,
}

/// One 16-byte TRB as four little-endian dwords.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Trb {
    pub(super) parameter: u64,
    pub(super) status: u32,
    pub(super) control: u32,
}

impl Trb {
    pub(super) const fn from_dwords(words: [u32; 4]) -> Self {
        Self {
            parameter: words[0] as u64 | (words[1] as u64) << 32,
            status: words[2],
            control: words[3],
        }
    }

    pub(super) const fn dwords(self) -> [u32; 4] {
        [
            self.parameter as u32,
            (self.parameter >> 32) as u32,
            self.status,
            self.control,
        ]
    }

    pub(super) const fn trb_type(self) -> u32 {
        (self.control >> 10) & 0x3f
    }

    pub(super) const fn cycle(self) -> bool {
        self.control & CYCLE != 0
    }

    const fn with_type(kind: u32, flags: u32) -> u32 {
        (kind << 10) | flags
    }

    pub(super) const fn setup_stage(packet: u64, transfer: TransferType) -> Self {
        Self {
            parameter: packet,
            status: 8,
            control: Self::with_type(
                trb_type::SETUP_STAGE,
                IMMEDIATE_DATA | ((transfer as u32) << 16),
            ),
        }
    }

    pub(super) const fn data_stage(buffer: u64, length: u32, device_to_host: bool) -> Self {
        Self {
            parameter: buffer,
            status: length,
            control: Self::with_type(
                trb_type::DATA_STAGE,
                if device_to_host { DIRECTION_IN } else { 0 },
            ),
        }
    }

    /// Status Stage with IOC: its Transfer Event ends the control transfer.
    pub(super) const fn status_stage(device_to_host: bool) -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: Self::with_type(
                trb_type::STATUS_STAGE,
                INTERRUPT_ON_COMPLETION | if device_to_host { DIRECTION_IN } else { 0 },
            ),
        }
    }

    /// Interrupt IN buffer: IOC and ISP, so a full or a short report completes the TRB.
    pub(super) const fn normal(buffer: u64, length: u32) -> Self {
        Self {
            parameter: buffer,
            status: length,
            control: Self::with_type(
                trb_type::NORMAL,
                INTERRUPT_ON_COMPLETION | INTERRUPT_ON_SHORT_PACKET,
            ),
        }
    }

    pub(super) const fn link(target: u64, toggle_cycle: bool) -> Self {
        Self {
            parameter: target,
            status: 0,
            control: Self::with_type(trb_type::LINK, if toggle_cycle { TOGGLE_CYCLE } else { 0 }),
        }
    }

    pub(super) const fn enable_slot() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: Self::with_type(trb_type::ENABLE_SLOT, 0),
        }
    }

    pub(super) const fn disable_slot(slot: u8) -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: Self::with_type(trb_type::DISABLE_SLOT, (slot as u32) << 24),
        }
    }

    pub(super) const fn address_device(input_context: u64, slot: u8) -> Self {
        Self {
            parameter: input_context,
            status: 0,
            // BSR = 0: the controller sends SET_ADDRESS.
            control: Self::with_type(trb_type::ADDRESS_DEVICE, (slot as u32) << 24)
                & !BLOCK_SET_ADDRESS,
        }
    }

    pub(super) const fn configure_endpoint(input_context: u64, slot: u8) -> Self {
        Self {
            parameter: input_context,
            status: 0,
            control: Self::with_type(trb_type::CONFIGURE_ENDPOINT, (slot as u32) << 24),
        }
    }

    // Event TRB fields.

    pub(super) const fn completion_code(self) -> u32 {
        self.status >> 24
    }

    pub(super) const fn slot_id(self) -> u8 {
        (self.control >> 24) as u8
    }

    pub(super) const fn endpoint_id(self) -> u8 {
        ((self.control >> 16) & 0x1f) as u8
    }

    /// Bytes not transferred (Transfer Event, status bits 23:0).
    pub(super) const fn residual(self) -> u32 {
        self.status & 0x00ff_ffff
    }
}

/// Producer side of a command or transfer ring: one segment whose last TRB is a Link TRB to
/// the start with Toggle Cycle (XHCI_DEVICE.md section 8).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RingCursor {
    base: u64,
    length: usize,
    index: usize,
    cycle: bool,
}

impl RingCursor {
    /// `length` counts TRBs including the Link TRB; at least 2.
    pub(super) const fn new(base: u64, length: usize) -> Self {
        Self {
            base,
            length,
            index: 0,
            cycle: true,
        }
    }

    pub(super) const fn base(&self) -> u64 {
        self.base
    }

    /// Producer cycle state that goes into the Dequeue Pointer's DCS bit.
    pub(super) const fn cycle(&self) -> bool {
        self.cycle
    }

    const fn address(&self, index: usize) -> u64 {
        self.base + index as u64 * TRB_BYTES
    }

    /// Hands out the slot for `trb`: calls `write(address, trb)` for the Link TRB (when the
    /// producer reached the end) and for the TRB itself, each with its cycle bit set. The
    /// caller must write dword 3 (the cycle bit) last. Returns the TRB address.
    pub(super) fn push(&mut self, mut trb: Trb, mut write: impl FnMut(u64, Trb)) -> u64 {
        if self.index == self.length - 1 {
            let mut link = Trb::link(self.base, true);
            link.control = (link.control & !CYCLE) | u32::from(self.cycle);
            write(self.address(self.index), link);
            self.index = 0;
            self.cycle = !self.cycle;
        }
        trb.control = (trb.control & !CYCLE) | u32::from(self.cycle);
        let address = self.address(self.index);
        write(address, trb);
        self.index += 1;
        address
    }
}

/// Consumer side of the single-segment event ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EventCursor {
    base: u64,
    length: usize,
    index: usize,
    cycle: bool,
}

impl EventCursor {
    pub(super) const fn new(base: u64, length: usize) -> Self {
        Self {
            base,
            length,
            index: 0,
            cycle: true,
        }
    }

    /// Address of the next TRB the controller will write.
    pub(super) const fn pending(&self) -> u64 {
        self.base + self.index as u64 * TRB_BYTES
    }

    pub(super) const fn owns(&self, trb: Trb) -> bool {
        trb.cycle() == self.cycle
    }

    pub(super) fn accept(&mut self) {
        self.index += 1;
        if self.index == self.length {
            self.index = 0;
            self.cycle = !self.cycle;
        }
    }
}

/// ERDP value with the Event Handler Busy bit set (write 1 clears EHB; XHCI_DEVICE.md
/// section 15: the firmware must write ERDP with EHB = 1).
pub(super) const fn erdp_value(dequeue: u64) -> u64 {
    (dequeue & !0xf) | (1 << 3)
}

/// Contexts are 32 bytes (CSZ = 0). Input Context: Input Control Context, Slot Context, then
/// Endpoint Contexts by DCI.
pub(super) const CONTEXT_BYTES: usize = 32;
pub(super) const INPUT_CONTEXT_BYTES: usize = 33 * CONTEXT_BYTES;
pub(super) const DEVICE_CONTEXT_BYTES: usize = 32 * CONTEXT_BYTES;

pub(super) const FULL_SPEED: u32 = 1;
pub(super) const EP_TYPE_CONTROL: u32 = 4;
pub(super) const EP_TYPE_INTERRUPT_IN: u32 = 7;
/// Control endpoint DCI and the keyboard's Interrupt IN DCI (XHCI_DEVICE.md section 8).
pub(super) const DCI_EP0: u8 = 1;
pub(super) const DCI_INTERRUPT_IN: u8 = 3;
pub(super) const EP0_MAX_PACKET: u32 = 64;

/// A built Input Context: dword words ready to be copied into guest RAM.
pub(super) struct InputContext(pub(super) [u32; INPUT_CONTEXT_BYTES / 4]);

impl InputContext {
    const fn slot_word(word: usize) -> usize {
        // Input Control Context occupies the first 8 dwords.
        8 + word
    }

    const fn endpoint_word(dci: u8, word: usize) -> usize {
        8 * (1 + dci as usize) + word
    }

    fn base(add_flags: u32, context_entries: u32, port: u8) -> Self {
        let mut words = [0u32; INPUT_CONTEXT_BYTES / 4];
        words[1] = add_flags;
        words[Self::slot_word(0)] = (context_entries << 27) | (FULL_SPEED << 20);
        words[Self::slot_word(1)] = u32::from(port) << 16;
        Self(words)
    }

    fn set_endpoint(
        &mut self,
        dci: u8,
        interval: u32,
        kind: u32,
        max_packet: u32,
        ring: &RingCursor,
        average_length: u32,
    ) {
        let first = Self::endpoint_word(dci, 0);
        self.0[first] = interval << 16;
        // CErr = 3, EP Type, Max Packet Size; Max Burst = 0.
        self.0[first + 1] = (3 << 1) | (kind << 3) | (max_packet << 16);
        let dequeue = ring.base() | u64::from(ring.cycle());
        self.0[first + 2] = dequeue as u32;
        self.0[first + 3] = (dequeue >> 32) as u32;
        self.0[first + 4] = average_length;
    }

    /// Address Device input (A0 = A1 = 1, Context Entries = 1, EP0 type Control).
    pub(super) fn address_device(port: u8, ep0: &RingCursor) -> Self {
        let mut context = Self::base(0b11, 1, port);
        context.set_endpoint(DCI_EP0, 0, EP_TYPE_CONTROL, EP0_MAX_PACKET, ep0, 8);
        context
    }

    /// Configure Endpoint input adding the Interrupt IN endpoint (A0 = A3 = 1, Context
    /// Entries = 3). `average_length` is also written as Max ESIT Payload.
    pub(super) fn configure_interrupt_in(
        port: u8,
        interval: u32,
        max_packet: u32,
        ring: &RingCursor,
    ) -> Self {
        let mut context = Self::base(
            1 | (1 << DCI_INTERRUPT_IN),
            u32::from(DCI_INTERRUPT_IN),
            port,
        );
        context.set_endpoint(
            DCI_INTERRUPT_IN,
            interval,
            EP_TYPE_INTERRUPT_IN,
            max_packet,
            ring,
            max_packet | (max_packet << 16),
        );
        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trb_dwords_round_trip_and_fields_decode() {
        let trb = Trb::from_dwords([0x1122_3344, 0x5566_7788, 0x0100_0008, 0x0301_8401]);
        assert_eq!(trb.parameter, 0x5566_7788_1122_3344);
        assert_eq!(
            trb.dwords(),
            [0x1122_3344, 0x5566_7788, 0x0100_0008, 0x0301_8401]
        );
        assert_eq!(trb.trb_type(), 0x21);
        assert!(trb.cycle());
        assert_eq!(trb.completion_code(), 1);
        assert_eq!(trb.residual(), 8);
        assert_eq!(trb.slot_id(), 3);
        assert_eq!(trb.endpoint_id(), 1);
    }

    #[test]
    fn transfer_trbs_use_the_documented_flags() {
        let setup = Trb::setup_stage(0x0008_0000_0100_0680, TransferType::In);
        assert_eq!(setup.trb_type(), trb_type::SETUP_STAGE);
        assert_eq!(setup.status, 8);
        assert_eq!(setup.control, (2 << 10) | (1 << 6) | (3 << 16));
        let out = Trb::setup_stage(0, TransferType::Out);
        assert_eq!((out.control >> 16) & 3, 2);
        assert_eq!(
            (Trb::setup_stage(0, TransferType::NoData).control >> 16) & 3,
            0
        );
        let data = Trb::data_stage(0x1000, 18, true);
        assert_eq!(data.control, (3 << 10) | (1 << 16));
        assert_eq!(data.status, 18);
        assert_eq!(Trb::data_stage(0x1000, 1, false).control, 3 << 10);
        // Status Stage: IOC; direction IN when there is no data or the data went out.
        assert_eq!(Trb::status_stage(false).control, (4 << 10) | (1 << 5));
        assert_eq!(
            Trb::status_stage(true).control,
            (4 << 10) | (1 << 5) | (1 << 16)
        );
        let normal = Trb::normal(0x2000, 8);
        assert_eq!(normal.control, (1 << 10) | (1 << 5) | (1 << 2));
        assert_eq!(normal.status, 8);
    }

    #[test]
    fn command_trbs_carry_type_slot_and_pointer() {
        assert_eq!(Trb::enable_slot().control, 9 << 10);
        assert_eq!(Trb::disable_slot(2).control, (10 << 10) | (2 << 24));
        let address = Trb::address_device(0x4_0000, 1);
        assert_eq!(address.parameter, 0x4_0000);
        // BSR (bit 9) stays 0: the controller sends SET_ADDRESS.
        assert_eq!(address.control, (11 << 10) | (1 << 24));
        let configure = Trb::configure_endpoint(0x4_0000, 1);
        assert_eq!(configure.control, (12 << 10) | (1 << 24));
        let link = Trb::link(0x8000, true);
        assert_eq!(link.control, (6 << 10) | 2);
        assert_eq!(link.parameter, 0x8000);
    }

    #[test]
    fn ring_cursor_sets_cycle_and_wraps_through_the_link_trb() {
        let mut ring = RingCursor::new(0x1_0000, 4);
        let mut written = Vec::new();
        for index in 0..5u32 {
            let address = ring.push(
                Trb {
                    parameter: u64::from(index),
                    status: 0,
                    control: trb_type::NORMAL << 10,
                },
                |address, trb| written.push((address, trb)),
            );
            if index < 3 {
                assert_eq!(address, 0x1_0000 + u64::from(index) * 16);
            }
        }
        // Three TRBs, the Link TRB with the old cycle, then the wrapped TRBs with cycle 0.
        assert_eq!(written.len(), 6);
        assert!(written[..3].iter().all(|(_, trb)| trb.cycle()));
        let (link_address, link) = written[3];
        assert_eq!(link_address, 0x1_0000 + 3 * 16);
        assert_eq!(link.trb_type(), trb_type::LINK);
        assert_eq!(link.parameter, 0x1_0000);
        assert!(link.cycle());
        assert_ne!(link.control & 2, 0);
        assert_eq!(written[4].0, 0x1_0000);
        assert!(!written[4].1.cycle());
        assert_eq!(written[5].0, 0x1_0000 + 16);
        assert!(!written[5].1.cycle());
        assert!(!ring.cycle());
    }

    #[test]
    fn event_cursor_follows_the_phase_bit() {
        let mut events = EventCursor::new(0x2_0000, 2);
        let fresh = Trb::from_dwords([0, 0, 0, 1 | (33 << 10)]);
        let stale = Trb::from_dwords([0, 0, 0, 33 << 10]);
        assert!(events.owns(fresh));
        assert!(!events.owns(stale));
        assert_eq!(events.pending(), 0x2_0000);
        events.accept();
        assert_eq!(events.pending(), 0x2_0010);
        events.accept();
        // Wrapped: the producer cycle is now 0.
        assert_eq!(events.pending(), 0x2_0000);
        assert!(events.owns(stale));
        assert!(!events.owns(fresh));
    }

    #[test]
    fn erdp_always_sets_event_handler_busy() {
        assert_eq!(erdp_value(0x2_0010), 0x2_0018);
        assert_eq!(erdp_value(0x2_0018), 0x2_0018);
    }

    #[test]
    fn address_device_input_context_matches_the_model_checks() {
        let ring = RingCursor::new(0x6000, 64);
        let context = InputContext::address_device(1, &ring).0;
        // Input Control Context: A0 and A1, nothing dropped.
        assert_eq!((context[0], context[1]), (0, 0b11));
        // Slot Context: Context Entries 1, Root Hub Port 1.
        assert_eq!(context[8] >> 27, 1);
        assert_eq!(context[9] >> 16 & 0xff, 1);
        // EP0 Context (DCI 1 at dword 16): Control, Max Packet 64, DCS 1, dequeue = ring base.
        assert_eq!(context[16 + 1] >> 3 & 7, EP_TYPE_CONTROL);
        assert_eq!(context[16 + 1] >> 16, 64);
        assert_eq!(context[16 + 2], 0x6001);
        assert_eq!(context[16 + 3], 0);
    }

    #[test]
    fn configure_endpoint_input_context_adds_dci_3() {
        let ring = RingCursor::new(0x7000, 64);
        let context = InputContext::configure_interrupt_in(1, 6, 8, &ring).0;
        assert_eq!(context[1], 0b1001);
        assert_eq!(context[8] >> 27, 3);
        // Endpoint Context for DCI 3 starts at dword 8 * (1 + 3).
        assert_eq!(context[32] >> 16, 6);
        assert_eq!(context[33] >> 3 & 7, EP_TYPE_INTERRUPT_IN);
        assert_eq!(context[33] >> 16, 8);
        assert_eq!(context[33] >> 8 & 0xff, 0);
        assert_eq!(context[34], 0x7001);
        assert_eq!(context[36], 8 | (8 << 16));
    }
}
