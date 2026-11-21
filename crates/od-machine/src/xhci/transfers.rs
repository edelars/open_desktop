//! Transfer rings of EP0 (control) and the Interrupt IN endpoint (`docs/specs/XHCI_DEVICE.md`
//! sections 9.2 and 9.3). A transfer descriptor (TD) is executed whole inside `service`: it is first
//! scanned (reads only: Link TRBs, chains, validation), then executed against the device and guest
//! buffers, and its events are planned; the controller posts them and moves the endpoint.

use super::device::{ControlResponse, InterruptStatus, UsbDevice};
use super::slots::Endpoint;
use super::trb::{
    CC_BABBLE_DETECTED_ERROR, CC_DATA_BUFFER_ERROR, CC_SHORT_PACKET, CC_STALL_ERROR, CC_SUCCESS,
    CC_TRB_ERROR, CC_USB_TRANSACTION_ERROR, TRB_BYTES, TRB_TYPE_DATA_STAGE, TRB_TYPE_LINK,
    TRB_TYPE_NORMAL, TRB_TYPE_SETUP_STAGE, TRB_TYPE_STATUS_STAGE, Trb,
};
use crate::nvme::GuestDma;

/// TRBs (Link TRBs included) one ring may consume in one `service` call.
pub(super) const TRANSFER_BUDGET: u32 = 256;
/// Longest chain of a Data Stage or an Interrupt IN TD (contract section 9.3).
const MAX_CHAIN: usize = 64;
/// Largest TD the 17-bit lengths and the 16-bit wLength allow.
const MAX_TD_BYTES: u32 = 65_535;

/// A TRB with the address it was read from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Located {
    pub(super) address: u64,
    pub(super) trb: Trb,
}

/// A place in a transfer ring: the dequeue pointer and the cycle state expected there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Position {
    pub(super) pointer: u64,
    pub(super) dcs: bool,
}

impl Position {
    /// The ring position of a TRB that was read with its own cycle bit.
    fn of(located: &Located) -> Self {
        Self {
            pointer: located.address,
            dcs: located.trb.cycle(),
        }
    }
}

/// Why scanning stopped before it knew what the ring holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScanStop {
    /// A TRB is not in guest RAM: a system error.
    Dma,
    /// The budget of this `service` call is spent.
    Budget,
}

/// The result of reading the ring at the dequeue pointer.
#[derive(Debug)]
pub(super) enum Scan {
    /// No TRB with the expected cycle bit: nothing to do.
    Empty,
    /// The TD is not fully queued yet (a stage or a chained TRB is missing).
    Incomplete,
    Td(Td),
    /// A TRB the model rejects: TRB Error, the endpoint goes to Error.
    Invalid(Located),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TdKind {
    Control,
    InterruptIn,
}

/// A complete TD: for control, the Setup Stage, the Data Stage chain (maybe empty) and the Status
/// Stage; for Interrupt IN, only the chain.
#[derive(Debug)]
pub(super) struct Td {
    kind: TdKind,
    start: Position,
    end: Position,
    setup: Option<Located>,
    data: Vec<Located>,
    status: Option<Located>,
}

impl Td {
    /// Upper bound of the events the TD can produce: one per IOC TRB and one for an error or a
    /// short packet on a TRB without IOC.
    pub(super) fn event_bound(&self) -> u32 {
        let iocs = self
            .setup
            .iter()
            .chain(&self.data)
            .chain(&self.status)
            .filter(|located| located.trb.ioc())
            .count();
        iocs as u32 + 1
    }

    pub(super) fn kind(&self) -> TdKind {
        self.kind
    }

    fn total_length(&self) -> u32 {
        self.data.iter().map(|located| located.trb.length()).sum()
    }
}

/// An event a TD produced: the TRB, the completion code and the bytes not transferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PlannedEvent {
    pub(super) address: u64,
    pub(super) code: u32,
    pub(super) remaining: u32,
    /// BEI of the TRB: the event raises no interrupt.
    pub(super) block_interrupt: bool,
}

impl PlannedEvent {
    fn new(located: &Located, code: u32, remaining: u32) -> Self {
        Self {
            address: located.address,
            code,
            remaining,
            // BEI is defined for Normal TRBs only (reserved in the stage TRBs).
            block_interrupt: located.trb.trb_type() == TRB_TYPE_NORMAL && located.trb.bei(),
        }
    }
}

/// What happens to the endpoint after a TD.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Disposition {
    /// The TD completed: the dequeue pointer moves behind it.
    Done(Position),
    /// A transfer error other than TRB Error: Halted at this TRB.
    Halt(Position),
    /// TRB Error: Error at this TRB.
    Fail(Position),
    /// The device NAKed: the TD stays queued and is tried again by the next `service`.
    Retry,
}

#[derive(Debug)]
pub(super) struct Execution {
    pub(super) events: Vec<PlannedEvent>,
    pub(super) disposition: Disposition,
}

impl Execution {
    /// TRB Error for a TRB the scan rejected.
    pub(super) fn trb_error(located: &Located) -> Self {
        let remaining = match located.trb.trb_type() {
            TRB_TYPE_NORMAL | TRB_TYPE_DATA_STAGE => located.trb.length(),
            _ => 0,
        };
        Self {
            events: vec![PlannedEvent::new(located, CC_TRB_ERROR, remaining)],
            disposition: Disposition::Fail(Position::of(located)),
        }
    }

    /// TRB Error for a ring the controller cannot get through (a chain of Link TRBs longer than
    /// the budget): reported at the dequeue pointer.
    pub(super) fn trb_error_at(position: Position) -> Self {
        Self {
            events: vec![PlannedEvent {
                address: position.pointer,
                code: CC_TRB_ERROR,
                remaining: 0,
                block_interrupt: false,
            }],
            disposition: Disposition::Fail(position),
        }
    }

    fn halt(events: Vec<PlannedEvent>, at: &Located) -> Self {
        Self {
            events,
            disposition: Disposition::Halt(Position::of(at)),
        }
    }
}

/// Reads TRBs from a ring, following Link TRBs, within a budget.
struct Cursor {
    pointer: u64,
    dcs: bool,
    used: u32,
    limit: u32,
}

impl Cursor {
    fn position(&self) -> Position {
        Position {
            pointer: self.pointer,
            dcs: self.dcs,
        }
    }

    /// Follows Link TRBs that are next in the ring, so that a dequeue pointer taken now names a
    /// real TRB (or the first TRB not yet written), not a Link TRB. Stops quietly on a read error
    /// or when the budget is spent.
    fn settle(&mut self, dma: &mut dyn GuestDma) {
        while self.used < self.limit {
            let mut raw = [0u8; 16];
            if dma.read(self.pointer, &mut raw).is_err() {
                return;
            }
            let trb = Trb::from_bytes(&raw);
            if trb.cycle() != self.dcs || trb.trb_type() != TRB_TYPE_LINK {
                return;
            }
            self.used += 1;
            self.dcs ^= trb.toggle_cycle();
            self.pointer = trb.parameter() & !0xF;
        }
    }

    /// The next TRB that is not a Link TRB, or `None` when the ring ends (cycle bit mismatch).
    fn next(&mut self, dma: &mut dyn GuestDma) -> Result<Option<Located>, ScanStop> {
        loop {
            if self.used >= self.limit {
                return Err(ScanStop::Budget);
            }
            let mut raw = [0u8; 16];
            dma.read(self.pointer, &mut raw)
                .map_err(|_| ScanStop::Dma)?;
            let trb = Trb::from_bytes(&raw);
            if trb.cycle() != self.dcs {
                return Ok(None);
            }
            self.used += 1;
            if trb.trb_type() == TRB_TYPE_LINK {
                self.dcs ^= trb.toggle_cycle();
                self.pointer = trb.parameter() & !0xF;
                continue;
            }
            let located = Located {
                address: self.pointer,
                trb,
            };
            self.pointer = self.pointer.wrapping_add(TRB_BYTES);
            return Ok(Some(located));
        }
    }
}

/// The bytes of a Setup Stage (bmRequestType, bRequest, wValue, wIndex, wLength).
fn setup_bytes(setup: &Trb) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&setup.0[0].to_le_bytes());
    bytes[4..].copy_from_slice(&setup.0[1].to_le_bytes());
    bytes
}

/// SET_ADDRESS is the controller's job (Address Device): a Setup Stage with it is a TRB Error
/// (xHCI 4.11.2.2).
fn is_set_address(setup: &Trb) -> bool {
    let bytes = setup_bytes(setup);
    bytes[0] == 0x00 && bytes[1] == 0x05
}

/// Reads the chain after `first`: Normal TRBs while CH = 1, at most [`MAX_CHAIN`] in all.
/// `Ok(Err(..))` is an invalid TRB, `Ok(Ok(None))` an incomplete chain.
fn read_chain(
    dma: &mut dyn GuestDma,
    cursor: &mut Cursor,
    first: Located,
) -> Result<Result<Option<Vec<Located>>, Located>, ScanStop> {
    let mut chain = vec![first];
    while chain.last().is_some_and(|last| last.trb.chain()) {
        let Some(next) = cursor.next(dma)? else {
            return Ok(Ok(None));
        };
        if next.trb.trb_type() != TRB_TYPE_NORMAL
            || next.trb.interrupter_target() != 0
            || next.trb.idt()
            || chain.len() >= MAX_CHAIN
        {
            return Ok(Err(next));
        }
        chain.push(next);
    }
    let total: u32 = chain.iter().map(|located| located.trb.length()).sum();
    if total > MAX_TD_BYTES {
        return Ok(Err(first));
    }
    Ok(Ok(Some(chain)))
}

/// Scans a control TD at the endpoint's dequeue pointer.
pub(super) fn scan_control(
    dma: &mut dyn GuestDma,
    endpoint: &Endpoint,
    budget: &mut u32,
) -> Result<Scan, ScanStop> {
    let mut cursor = Cursor {
        pointer: endpoint.dequeue,
        dcs: endpoint.dcs,
        used: 0,
        limit: *budget,
    };
    cursor.settle(dma);
    let start = cursor.position();
    let scan = scan_control_inner(dma, &mut cursor, start);
    *budget -= cursor.used.min(*budget);
    scan
}

fn scan_control_inner(
    dma: &mut dyn GuestDma,
    cursor: &mut Cursor,
    start: Position,
) -> Result<Scan, ScanStop> {
    let Some(setup) = cursor.next(dma)? else {
        return Ok(Scan::Empty);
    };
    let trb = &setup.trb;
    // Setup Stage: immediate data, 8 bytes, not chained, Interrupter 0; Event Data and
    // Transfer No-Op are v1 deviations and end here as TRB Error too.
    if trb.trb_type() != TRB_TYPE_SETUP_STAGE
        || !trb.idt()
        || trb.length() != 8
        || trb.chain()
        || trb.interrupter_target() != 0
        || is_set_address(trb)
    {
        return Ok(Scan::Invalid(setup));
    }
    let Some(first) = cursor.next(dma)? else {
        return Ok(Scan::Incomplete);
    };
    let (data, status) = match first.trb.trb_type() {
        TRB_TYPE_DATA_STAGE => {
            // Immediate data is OUT data of at most 8 bytes in one TRB.
            if first.trb.interrupter_target() != 0
                || (first.trb.idt()
                    && (first.trb.dir_in() || first.trb.chain() || first.trb.length() > 8))
            {
                return Ok(Scan::Invalid(first));
            }
            let chain = match read_chain(dma, cursor, first)? {
                Err(invalid) => return Ok(Scan::Invalid(invalid)),
                Ok(None) => return Ok(Scan::Incomplete),
                Ok(Some(chain)) => chain,
            };
            let Some(status) = cursor.next(dma)? else {
                return Ok(Scan::Incomplete);
            };
            (chain, status)
        }
        TRB_TYPE_STATUS_STAGE => (Vec::new(), first),
        _ => return Ok(Scan::Invalid(first)),
    };
    if status.trb.trb_type() != TRB_TYPE_STATUS_STAGE
        || status.trb.chain()
        || status.trb.interrupter_target() != 0
    {
        return Ok(Scan::Invalid(status));
    }
    cursor.settle(dma);
    Ok(Scan::Td(Td {
        kind: TdKind::Control,
        start,
        end: cursor.position(),
        setup: Some(setup),
        data,
        status: Some(status),
    }))
}

/// Scans an Interrupt IN TD: a chain of Normal TRBs.
pub(super) fn scan_interrupt(
    dma: &mut dyn GuestDma,
    endpoint: &Endpoint,
    budget: &mut u32,
) -> Result<Scan, ScanStop> {
    let mut cursor = Cursor {
        pointer: endpoint.dequeue,
        dcs: endpoint.dcs,
        used: 0,
        limit: *budget,
    };
    cursor.settle(dma);
    let start = cursor.position();
    let scan = scan_interrupt_inner(dma, &mut cursor, start);
    *budget -= cursor.used.min(*budget);
    scan
}

fn scan_interrupt_inner(
    dma: &mut dyn GuestDma,
    cursor: &mut Cursor,
    start: Position,
) -> Result<Scan, ScanStop> {
    let Some(first) = cursor.next(dma)? else {
        return Ok(Scan::Empty);
    };
    if first.trb.trb_type() != TRB_TYPE_NORMAL
        || first.trb.interrupter_target() != 0
        || first.trb.idt()
    {
        return Ok(Scan::Invalid(first));
    }
    let chain = match read_chain(dma, cursor, first)? {
        Err(invalid) => return Ok(Scan::Invalid(invalid)),
        Ok(None) => return Ok(Scan::Incomplete),
        Ok(Some(chain)) => chain,
    };
    cursor.settle(dma);
    Ok(Scan::Td(Td {
        kind: TdKind::InterruptIn,
        start,
        end: cursor.position(),
        setup: None,
        data: chain,
        status: None,
    }))
}

/// The events of data that ended after `transferred` of the chain's bytes (contract 9.3).
/// A full transfer gives Success for every TRB with IOC. A short transfer gives, for the TRB the
/// data ended in, an event if it has ISP (or IOC), and for every later TRB with IOC an event with
/// the same code and remaining count. Control short packets are code 13 with ISP or IOC;
/// Interrupt IN reports short without ISP complete with Success and the remainder.
fn data_events(chain: &[Located], transferred: u32, control: bool) -> Vec<PlannedEvent> {
    let total: u32 = chain.iter().map(|located| located.trb.length()).sum();
    if transferred >= total {
        return chain
            .iter()
            .filter(|located| located.trb.ioc())
            .map(|located| PlannedEvent::new(located, CC_SUCCESS, 0))
            .collect();
    }
    let mut before = 0;
    let mut short_index = 0;
    let mut remaining = 0;
    for (index, located) in chain.iter().enumerate() {
        let through = before + located.trb.length();
        if transferred <= through {
            short_index = index;
            remaining = through - transferred;
            break;
        }
        before = through;
    }
    let short = &chain[short_index];
    // Control: the remainder of the TRB the data ended in. Interrupt IN (contract 9.3): the
    // remainder of the whole TD.
    let remaining = if control {
        remaining
    } else {
        total - transferred
    };
    let code = if control || short.trb.isp() {
        CC_SHORT_PACKET
    } else {
        CC_SUCCESS
    };
    // TRBs that were filled completely before the short one complete normally.
    let mut events: Vec<PlannedEvent> = chain[..short_index]
        .iter()
        .filter(|located| located.trb.ioc())
        .map(|located| PlannedEvent::new(located, CC_SUCCESS, 0))
        .collect();
    if short.trb.isp() || short.trb.ioc() {
        events.push(PlannedEvent::new(short, code, remaining));
    }
    events.extend(
        chain[short_index + 1..]
            .iter()
            .filter(|located| located.trb.ioc())
            .map(|located| PlannedEvent::new(located, code, remaining)),
    );
    events
}

/// Writes `data` into the chain's buffers in order. `Err(index)` is the TRB whose buffer is not RAM.
fn write_chain(dma: &mut dyn GuestDma, chain: &[Located], data: &[u8]) -> Result<(), usize> {
    let mut offset = 0;
    for (index, located) in chain.iter().enumerate() {
        if offset == data.len() {
            break;
        }
        let length = (located.trb.length() as usize).min(data.len() - offset);
        if length == 0 {
            // A TRB with Length 0 takes no data; the chain goes on.
            continue;
        }
        dma.write(located.trb.parameter(), &data[offset..offset + length])
            .map_err(|_| index)?;
        offset += length;
    }
    Ok(())
}

/// Reads the OUT data of a Data Stage (immediate data comes from the TRB itself).
fn read_chain_data(dma: &mut dyn GuestDma, chain: &[Located]) -> Result<Vec<u8>, usize> {
    let mut data = Vec::new();
    for (index, located) in chain.iter().enumerate() {
        let length = located.trb.length() as usize;
        if located.trb.idt() {
            let mut immediate = [0u8; 8];
            immediate[..4].copy_from_slice(&located.trb.0[0].to_le_bytes());
            immediate[4..].copy_from_slice(&located.trb.0[1].to_le_bytes());
            data.extend_from_slice(&immediate[..length.min(8)]);
        } else if length > 0 {
            let mut buffer = vec![0u8; length];
            dma.read(located.trb.parameter(), &mut buffer)
                .map_err(|_| index)?;
            data.extend_from_slice(&buffer);
        }
    }
    Ok(data)
}

/// Executes a scanned control TD. `usable` is false when the device is not in the state the
/// slot's context expects (after a port reset, or behind a disabled port).
pub(super) fn execute_control(
    dma: &mut dyn GuestDma,
    td: &Td,
    device: &mut dyn UsbDevice,
    usable: bool,
) -> Execution {
    let (Some(setup), Some(status)) = (td.setup, td.status) else {
        unreachable!("a control TD has a Setup Stage and a Status Stage");
    };
    if !usable {
        return Execution {
            events: vec![PlannedEvent::new(&setup, CC_USB_TRANSACTION_ERROR, 0)],
            disposition: Disposition::Halt(td.start),
        };
    }
    let mut events = Vec::new();
    if setup.trb.ioc() {
        events.push(PlannedEvent::new(&setup, CC_SUCCESS, 0));
    }
    let total = td.total_length();
    let data_in = td.data.first().is_some_and(|first| first.trb.dir_in());
    // The TRB errors of the data stage land on its first TRB; without data, on the Status Stage.
    let first_stage = td.data.first().copied().unwrap_or(status);
    let out_data = if data_in {
        Vec::new()
    } else {
        match read_chain_data(dma, &td.data) {
            Ok(data) => data,
            Err(index) => {
                let bad = &td.data[index];
                events.push(PlannedEvent::new(
                    bad,
                    CC_DATA_BUFFER_ERROR,
                    bad.trb.length(),
                ));
                return Execution::halt(events, bad);
            }
        }
    };
    let setup_packet = setup_bytes(&setup.trb);
    let response = device.control(&setup_packet, &out_data);
    let mut transferred = total;
    if let ControlResponse::Stall = response {
        events.push(PlannedEvent::new(&first_stage, CC_STALL_ERROR, total));
        return Execution::halt(events, &first_stage);
    }
    if data_in {
        let reply = match response {
            ControlResponse::Data(bytes) => bytes,
            _ => Vec::new(),
        };
        let w_length = u32::from(u16::from_le_bytes([setup_packet[6], setup_packet[7]]));
        let length = (total.min(w_length) as usize).min(reply.len());
        transferred = length as u32;
        if let Err(index) = write_chain(dma, &td.data, &reply[..length]) {
            let bad = &td.data[index];
            events.push(PlannedEvent::new(
                bad,
                CC_DATA_BUFFER_ERROR,
                bad.trb.length(),
            ));
            return Execution::halt(events, bad);
        }
    }
    if !td.data.is_empty() {
        events.extend(data_events(&td.data, transferred, true));
    }
    if status.trb.ioc() {
        events.push(PlannedEvent::new(&status, CC_SUCCESS, 0));
    }
    Execution {
        events,
        disposition: Disposition::Done(td.end),
    }
}

/// Executes a scanned Interrupt IN TD: polls the device, and when a report is ready writes it
/// into the chain. A NAK leaves the TD queued.
pub(super) fn execute_interrupt(
    dma: &mut dyn GuestDma,
    td: &Td,
    device: &mut dyn UsbDevice,
    usable: bool,
) -> Execution {
    let first = td.data[0];
    let total = td.total_length();
    if !usable {
        return Execution {
            events: vec![PlannedEvent::new(&first, CC_USB_TRANSACTION_ERROR, total)],
            disposition: Disposition::Halt(td.start),
        };
    }
    match device.interrupt_in() {
        InterruptStatus::Nak => Execution {
            events: Vec::new(),
            disposition: Disposition::Retry,
        },
        InterruptStatus::Stall => Execution {
            events: vec![PlannedEvent::new(&first, CC_STALL_ERROR, total)],
            disposition: Disposition::Halt(td.start),
        },
        InterruptStatus::Report(length) if length as u32 > total => Execution {
            // The report stays in the device: the driver recovers the endpoint and requeues.
            events: vec![PlannedEvent::new(&first, CC_BABBLE_DETECTED_ERROR, total)],
            disposition: Disposition::Halt(td.start),
        },
        InterruptStatus::Report(_) => {
            let report = device.take_interrupt_report();
            if report.len() as u32 > total {
                // The device sent more than it announced (or the TD is too small after all).
                return Execution {
                    events: vec![PlannedEvent::new(&first, CC_BABBLE_DETECTED_ERROR, total)],
                    disposition: Disposition::Halt(td.start),
                };
            }
            if let Err(index) = write_chain(dma, &td.data, &report) {
                let bad = &td.data[index];
                return Execution {
                    events: vec![PlannedEvent::new(
                        bad,
                        CC_DATA_BUFFER_ERROR,
                        bad.trb.length(),
                    )],
                    disposition: Disposition::Halt(Position::of(bad)),
                };
            }
            Execution {
                events: data_events(&td.data, report.len() as u32, false),
                disposition: Disposition::Done(td.end),
            }
        }
    }
}
