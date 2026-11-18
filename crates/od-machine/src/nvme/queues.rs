//! Submission/completion queue state, command and completion entry layouts,
//! NVMe status values and the guest-memory (DMA) access used for PRP transfers.
//!
//! The specification text could not be extracted in the session that wrote
//! this file (see `docs/specs/NVME_DEVICE.md`, section 1). Cross-checked with
//! Linux `include/linux/nvme.h` (master, 2026-11-07), not with the text of
//! NVMe Base 2.4: opcodes, I/O Flush/Write/Read 0/1/2, generic statuses
//! 0x01/02/04/0B/0C/13 and command specific 0x100/101/102/105/108/109/10C.
//! `NOT VERIFIED`: PRP rules, AER info 0/1, error behaviour, which commands
//! are mandatory. Slice B adds the I/O statuses `LBA Out of Range` (0x80),
//! `Namespace Not Ready` (0x82) and `Internal Error` (0x06): the values are
//! cross-checked with `nvme.h`; the DNR bit and the conditions they are used
//! for are `NOT VERIFIED against NVMe Base 2.4`.

use crate::{GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MemoryRegionKind};

/// Submission queue entry size (CC.IOSQES = 6 is the only accepted value).
pub(crate) const SQE_BYTES: usize = 64;
/// Completion queue entry size (CC.IOCQES = 4 is the only accepted value).
pub(crate) const CQE_BYTES: usize = 16;
/// Memory page size: CC.MPS = 0 is the only accepted value (CAP.MPSMIN = MPSMAX = 0).
pub(crate) const PAGE_BYTES: u64 = 4096;
/// Largest number of entries of any queue (CAP.MQES + 1).
pub(crate) const MAX_QUEUE_ENTRIES: u32 = 1024;

/// NVMe status: status code type and status code of a completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Status {
    pub(crate) sct: u8,
    pub(crate) sc: u8,
    pub(crate) dnr: bool,
}

impl Status {
    const fn generic(sc: u8, dnr: bool) -> Self {
        Self { sct: 0, sc, dnr }
    }
    const fn command_specific(sc: u8) -> Self {
        Self {
            sct: 1,
            sc,
            dnr: true,
        }
    }

    pub(crate) const SUCCESS: Self = Self::generic(0x00, false);
    pub(crate) const INVALID_OPCODE: Self = Self::generic(0x01, true);
    pub(crate) const INVALID_FIELD: Self = Self::generic(0x02, true);
    pub(crate) const DATA_TRANSFER_ERROR: Self = Self::generic(0x04, false);
    pub(crate) const COMMAND_SEQUENCE_ERROR: Self = Self::generic(0x0C, true);
    pub(crate) const INVALID_NAMESPACE: Self = Self::generic(0x0B, true);
    // Value cross-checked with Linux nvme.h (LBA Out of Range 0x80); DNR and
    // the condition it is used for: NOT VERIFIED against NVMe Base 2.4.
    pub(crate) const LBA_OUT_OF_RANGE: Self = Self::generic(0x80, true);
    // Values cross-checked with Linux nvme.h (Namespace Not Ready 0x82,
    // Internal Error 0x06 for a backend failure); DNR (0 here, because the
    // condition may pass) and the conditions: NOT VERIFIED against NVMe Base 2.4.
    pub(crate) const INTERNAL_ERROR: Self = Self::generic(0x06, false);
    pub(crate) const NAMESPACE_NOT_READY: Self = Self::generic(0x82, false);
    pub(crate) const PRP_OFFSET_INVALID: Self = Self::generic(0x13, true);
    pub(crate) const COMPLETION_QUEUE_INVALID: Self = Self::command_specific(0x00);
    pub(crate) const INVALID_QUEUE_IDENTIFIER: Self = Self::command_specific(0x01);
    pub(crate) const INVALID_QUEUE_SIZE: Self = Self::command_specific(0x02);
    pub(crate) const AER_LIMIT_EXCEEDED: Self = Self::command_specific(0x05);
    pub(crate) const INVALID_INTERRUPT_VECTOR: Self = Self::command_specific(0x08);
    pub(crate) const INVALID_LOG_PAGE: Self = Self::command_specific(0x09);
    pub(crate) const INVALID_QUEUE_DELETION: Self = Self::command_specific(0x0C);

    pub(crate) const fn is_success(self) -> bool {
        self.sct == 0 && self.sc == 0
    }

    /// Bits `[15:1]` of the CQE `DW3[31:16]` status field (without the phase tag).
    pub(crate) const fn field(self) -> u16 {
        ((self.sc as u16) | ((self.sct as u16) << 8) | ((self.dnr as u16) << 14)) << 1
    }
}

/// Result of executing one command: completion dword 0 and status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Completion {
    pub(crate) dw0: u32,
    pub(crate) status: Status,
}

impl Completion {
    pub(crate) const fn ok(dw0: u32) -> Self {
        Self {
            dw0,
            status: Status::SUCCESS,
        }
    }
    pub(crate) const fn error(status: Status) -> Self {
        Self { dw0: 0, status }
    }
}

/// What executing a command did: it completed, or it stays outstanding
/// (an `Asynchronous Event Request` waiting for an event).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Done(Completion),
    Deferred,
}

/// A decoded 64-byte submission queue entry (common dwords 0, 1, 6-15).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Command {
    pub(crate) opcode: u8,
    pub(crate) fuse: u8,
    pub(crate) psdt: u8,
    pub(crate) cid: u16,
    pub(crate) nsid: u32,
    pub(crate) prp1: u64,
    pub(crate) prp2: u64,
    pub(crate) cdw10: u32,
    pub(crate) cdw11: u32,
    pub(crate) cdw12: u32,
    pub(crate) cdw13: u32,
}

impl Command {
    pub(crate) fn decode(raw: &[u8; SQE_BYTES]) -> Self {
        let dword = |index: usize| {
            u32::from_le_bytes([
                raw[index * 4],
                raw[index * 4 + 1],
                raw[index * 4 + 2],
                raw[index * 4 + 3],
            ])
        };
        let qword = |index: usize| u64::from(dword(index)) | (u64::from(dword(index + 1)) << 32);
        let dw0 = dword(0);
        Self {
            opcode: (dw0 & 0xFF) as u8,
            fuse: ((dw0 >> 8) & 0x3) as u8,
            psdt: ((dw0 >> 14) & 0x3) as u8,
            cid: (dw0 >> 16) as u16,
            nsid: dword(1),
            prp1: qword(6),
            prp2: qword(8),
            cdw10: dword(10),
            cdw11: dword(11),
            cdw12: dword(12),
            cdw13: dword(13),
        }
    }
}

/// Encodes a completion queue entry; `phase` is the queue's current phase tag.
pub(crate) fn encode_cqe(
    completion: Completion,
    sq_head: u16,
    sq_id: u16,
    cid: u16,
    phase: bool,
) -> [u8; CQE_BYTES] {
    let dw2 = u32::from(sq_head) | (u32::from(sq_id) << 16);
    let dw3 =
        u32::from(cid) | (u32::from(phase) << 16) | (u32::from(completion.status.field()) << 16);
    let mut raw = [0u8; CQE_BYTES];
    raw[0..4].copy_from_slice(&completion.dw0.to_le_bytes());
    raw[8..12].copy_from_slice(&dw2.to_le_bytes());
    raw[12..16].copy_from_slice(&dw3.to_le_bytes());
    raw
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SubmissionQueue {
    pub(crate) base: u64,
    pub(crate) entries: u16,
    pub(crate) head: u16,
    pub(crate) tail: u16,
    pub(crate) cqid: u16,
}

impl SubmissionQueue {
    pub(crate) const fn new(base: u64, entries: u16, cqid: u16) -> Self {
        Self {
            base,
            entries,
            head: 0,
            tail: 0,
            cqid,
        }
    }
    pub(crate) const fn has_work(&self) -> bool {
        self.head != self.tail
    }
    /// Number of commands covered by the current tail doorbell.
    pub(crate) const fn pending_entries(&self) -> u16 {
        (self.tail + self.entries - self.head) % self.entries
    }
    /// Guest address of the entry at `head`.
    pub(crate) fn head_address(&self) -> u64 {
        self.base + u64::from(self.head) * SQE_BYTES as u64
    }
    pub(crate) const fn advance_head(&mut self) {
        self.head = (self.head + 1) % self.entries;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompletionQueue {
    pub(crate) base: u64,
    pub(crate) entries: u16,
    /// Last head value the host wrote to the doorbell.
    pub(crate) head: u16,
    /// Next slot the controller writes.
    pub(crate) tail: u16,
    /// Current phase tag; starts at 1 and flips on every wrap of `tail`.
    pub(crate) phase: bool,
    /// Interrupt enable (`IEN`); the admin CQ has interrupts enabled implicitly.
    pub(crate) interrupt_enabled: bool,
}

impl CompletionQueue {
    pub(crate) const fn new(base: u64, entries: u16, interrupt_enabled: bool) -> Self {
        Self {
            base,
            entries,
            head: 0,
            tail: 0,
            phase: true,
            interrupt_enabled,
        }
    }
    /// A queue with one free slot left is full: head == tail would be "empty".
    pub(crate) const fn is_full(&self) -> bool {
        (self.tail + 1) % self.entries == self.head
    }
    pub(crate) const fn requests_interrupt(&self) -> bool {
        self.interrupt_enabled && self.head != self.tail
    }
    /// Number of CQEs that can be posted before the ring becomes full.
    pub(crate) const fn free_entries(&self) -> u16 {
        (self.head + self.entries - self.tail - 1) % self.entries
    }
    pub(crate) fn tail_address(&self) -> u64 {
        self.base + u64::from(self.tail) * CQE_BYTES as u64
    }
    pub(crate) const fn advance_tail(&mut self) {
        self.tail += 1;
        if self.tail == self.entries {
            self.tail = 0;
            self.phase = !self.phase;
        }
    }
}

/// Failure of a guest-memory access made on behalf of a command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DmaError;

/// Guest physical memory as the controller sees it. Only RAM is accessible:
/// a PRP or queue address outside RAM is a data transfer error, never a panic.
pub(crate) trait GuestDma {
    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), DmaError>;
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), DmaError>;
}

/// [`GuestDma`] over the machine bus; writes report their range to `committed`
/// like every other guest-visible RAM write (code-page tracking).
pub(crate) struct BusDma<'a, F: FnMut(GuestPhysicalRange)> {
    pub(crate) bus: &'a mut MachineBus,
    pub(crate) committed: F,
}

impl<F: FnMut(GuestPhysicalRange)> BusDma<'_, F> {
    fn check_ram(&self, address: u64, len: usize) -> Result<GuestPhysicalAddress, DmaError> {
        let len = u64::try_from(len).map_err(|_| DmaError)?;
        if len == 0 || address.checked_add(len).is_none() {
            return Err(DmaError);
        }
        let start = GuestPhysicalAddress::new(address);
        match self.bus.memory_kind(start, len) {
            Ok(MemoryRegionKind::Ram) => Ok(start),
            _ => Err(DmaError),
        }
    }
}

impl<F: FnMut(GuestPhysicalRange)> GuestDma for BusDma<'_, F> {
    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), DmaError> {
        let start = self.check_ram(address, buffer.len())?;
        self.bus
            .read_flat_bytes(start, buffer)
            .map_err(|_| DmaError)
    }
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), DmaError> {
        let start = self.check_ram(address, data.len())?;
        self.bus
            .write_flat_bytes(start, data, &mut self.committed)
            .map_err(|_| DmaError)
    }
}

/// Upper bound on PRP entries of one command (1 MiB at 4 KiB pages is 256).
const MAX_PRP_PAGES: u64 = 1024;

/// Splits a data transfer of `len` bytes described by `prp1`/`prp2` into
/// contiguous guest segments (NVMe Base, "Physical Region Page Entry and
/// List"; NOT VERIFIED against NVMe Base 2.4). Supports PRP1 with an offset,
/// PRP2 as a second page or as a PRP list pointer, and chained list pages.
pub(crate) fn prp_segments(
    dma: &mut dyn GuestDma,
    prp1: u64,
    prp2: u64,
    len: usize,
) -> Result<Vec<(u64, usize)>, Status> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if prp1 & 3 != 0 {
        return Err(Status::INVALID_FIELD);
    }
    let mut remaining = len as u64;
    let first = remaining.min(PAGE_BYTES - prp1 % PAGE_BYTES);
    let mut segments = vec![(prp1, first as usize)];
    remaining -= first;
    if remaining == 0 {
        return Ok(segments);
    }
    if remaining <= PAGE_BYTES {
        if !prp2.is_multiple_of(PAGE_BYTES) {
            return Err(Status::PRP_OFFSET_INVALID);
        }
        segments.push((prp2, remaining as usize));
        return Ok(segments);
    }
    // PRP2 points to a PRP list; the pointer itself only has to be qword aligned.
    if prp2 & 7 != 0 {
        return Err(Status::PRP_OFFSET_INVALID);
    }
    let mut pages_left = remaining.div_ceil(PAGE_BYTES);
    if pages_left > MAX_PRP_PAGES {
        return Err(Status::INVALID_FIELD);
    }
    let mut list = prp2;
    while pages_left > 0 {
        let slots = (PAGE_BYTES - list % PAGE_BYTES) / 8;
        let chained = pages_left > slots;
        let data_entries = if chained { slots - 1 } else { pages_left };
        let read_entries = data_entries + u64::from(chained);
        let mut raw = vec![0u8; (read_entries * 8) as usize];
        dma.read(list, &mut raw)
            .map_err(|_| Status::DATA_TRANSFER_ERROR)?;
        let entry = |index: usize| {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&raw[index * 8..index * 8 + 8]);
            u64::from_le_bytes(bytes)
        };
        for index in 0..data_entries as usize {
            let page = entry(index);
            if page % PAGE_BYTES != 0 {
                return Err(Status::PRP_OFFSET_INVALID);
            }
            let take = remaining.min(PAGE_BYTES);
            segments.push((page, take as usize));
            remaining -= take;
        }
        pages_left -= data_entries;
        if chained {
            list = entry(data_entries as usize);
            if !list.is_multiple_of(PAGE_BYTES) {
                return Err(Status::PRP_OFFSET_INVALID);
            }
        }
    }
    Ok(segments)
}

/// Copies `data` (controller to host) into the guest buffer described by the PRPs.
pub(crate) fn write_to_host(
    dma: &mut dyn GuestDma,
    prp1: u64,
    prp2: u64,
    data: &[u8],
) -> Result<(), Status> {
    let segments = prp_segments(dma, prp1, prp2, data.len())?;
    let mut offset = 0usize;
    for (address, length) in segments {
        dma.write(address, &data[offset..offset + length])
            .map_err(|_| Status::DATA_TRANSFER_ERROR)?;
        offset += length;
    }
    Ok(())
}

/// Copies the guest buffer described by the PRPs (host to controller) into a new vector.
pub(crate) fn read_from_host(
    dma: &mut dyn GuestDma,
    prp1: u64,
    prp2: u64,
    len: usize,
) -> Result<Vec<u8>, Status> {
    let segments = prp_segments(dma, prp1, prp2, len)?;
    let mut data = vec![0u8; len];
    let mut offset = 0usize;
    for (address, length) in segments {
        dma.read(address, &mut data[offset..offset + length])
            .map_err(|_| Status::DATA_TRANSFER_ERROR)?;
        offset += length;
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Sparse guest memory for PRP tests; any address below `limit` is RAM.
    struct MockDma {
        bytes: BTreeMap<u64, u8>,
        limit: u64,
    }

    impl MockDma {
        fn new(limit: u64) -> Self {
            Self {
                bytes: BTreeMap::new(),
                limit,
            }
        }
        fn in_range(&self, address: u64, len: usize) -> bool {
            address
                .checked_add(len as u64)
                .is_some_and(|end| end <= self.limit)
        }
        fn put_qword(&mut self, address: u64, value: u64) {
            for (index, byte) in value.to_le_bytes().iter().enumerate() {
                self.bytes.insert(address + index as u64, *byte);
            }
        }
    }

    impl GuestDma for MockDma {
        fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), DmaError> {
            if !self.in_range(address, buffer.len()) {
                return Err(DmaError);
            }
            for (index, slot) in buffer.iter_mut().enumerate() {
                *slot = self
                    .bytes
                    .get(&(address + index as u64))
                    .copied()
                    .unwrap_or(0);
            }
            Ok(())
        }
        fn write(&mut self, address: u64, data: &[u8]) -> Result<(), DmaError> {
            if !self.in_range(address, data.len()) {
                return Err(DmaError);
            }
            for (index, byte) in data.iter().enumerate() {
                self.bytes.insert(address + index as u64, *byte);
            }
            Ok(())
        }
    }

    #[test]
    fn prp1_alone_covers_a_transfer_inside_one_page() {
        let mut dma = MockDma::new(1 << 20);
        let segments = prp_segments(&mut dma, 0x2000 + 0x100, 0, 0x200).unwrap();
        assert_eq!(segments, vec![(0x2100, 0x200)]);
    }

    #[test]
    fn prp2_is_the_second_page_when_the_transfer_crosses_one_boundary() {
        let mut dma = MockDma::new(1 << 20);
        let segments = prp_segments(&mut dma, 0x2000 + 0xE00, 0x9000, 0x400).unwrap();
        assert_eq!(segments, vec![(0x2E00, 0x200), (0x9000, 0x200)]);
    }

    #[test]
    fn unaligned_prp2_data_pointer_is_prp_offset_invalid() {
        let mut dma = MockDma::new(1 << 20);
        assert_eq!(
            prp_segments(&mut dma, 0x2E00, 0x9004, 0x400),
            Err(Status::PRP_OFFSET_INVALID)
        );
    }

    #[test]
    fn prp_list_is_followed_and_the_last_page_is_partial() {
        let mut dma = MockDma::new(1 << 20);
        // 0x3000 bytes starting 0x800 into the first page: 0x800 + 2*0x1000 + 0x800.
        dma.put_qword(0x7000, 0x8000);
        dma.put_qword(0x7008, 0x9000);
        dma.put_qword(0x7010, 0xA000);
        let segments = prp_segments(&mut dma, 0x2800, 0x7000, 0x3000).unwrap();
        assert_eq!(
            segments,
            vec![
                (0x2800, 0x800),
                (0x8000, 0x1000),
                (0x9000, 0x1000),
                (0xA000, 0x800)
            ]
        );
    }

    #[test]
    fn prp_list_chains_through_the_last_entry_of_a_page() {
        let mut dma = MockDma::new(1 << 24);
        // First page plus 513 listed pages: the first list page holds 511 data
        // entries and the chain pointer, the second list page holds the other two.
        let pages = 514u64;
        let list0 = 0x10_0000u64;
        let list1 = 0x10_1000u64;
        for index in 0..511u64 {
            dma.put_qword(list0 + index * 8, 0x20_0000 + index * 0x1000);
        }
        dma.put_qword(list0 + 511 * 8, list1);
        for index in 0..2u64 {
            dma.put_qword(list1 + index * 8, 0x40_0000 + index * 0x1000);
        }
        let len = (pages * PAGE_BYTES) as usize;
        let segments = prp_segments(&mut dma, 0x2000, list0, len).unwrap();
        assert_eq!(segments.len(), 514);
        assert_eq!(segments[1], (0x20_0000, 0x1000));
        assert_eq!(segments[511], (0x20_0000 + 510 * 0x1000, 0x1000));
        assert_eq!(segments[512], (0x40_0000, 0x1000));
        assert_eq!(segments[513], (0x40_0000 + 0x1000, 0x1000));
    }

    #[test]
    fn list_entry_that_is_not_page_aligned_is_rejected() {
        let mut dma = MockDma::new(1 << 20);
        dma.put_qword(0x7000, 0x8000);
        dma.put_qword(0x7008, 0x9010);
        assert_eq!(
            prp_segments(&mut dma, 0x2000, 0x7000, 0x3000),
            Err(Status::PRP_OFFSET_INVALID)
        );
    }

    #[test]
    fn list_outside_ram_is_a_data_transfer_error() {
        let mut dma = MockDma::new(0x8000);
        assert_eq!(
            prp_segments(&mut dma, 0x2000, 0x9000, 0x3000),
            Err(Status::DATA_TRANSFER_ERROR)
        );
    }

    #[test]
    fn dword_misaligned_prp1_is_invalid_field() {
        let mut dma = MockDma::new(1 << 20);
        assert_eq!(
            prp_segments(&mut dma, 0x2002, 0, 16),
            Err(Status::INVALID_FIELD)
        );
    }

    #[test]
    fn write_to_host_spans_pages_and_reports_unmapped_targets() {
        let mut dma = MockDma::new(0x1_0000);
        let data: Vec<u8> = (0..0x300).map(|index| index as u8).collect();
        write_to_host(&mut dma, 0x2F00, 0x9000, &data).unwrap();
        assert_eq!(dma.bytes[&0x2F00], data[0]);
        assert_eq!(dma.bytes[&0x2FFF], data[0xFF]);
        assert_eq!(dma.bytes[&0x9000], data[0x100]);
        assert_eq!(dma.bytes[&(0x9000 + 0x1FF)], data[0x2FF]);
        assert_eq!(
            write_to_host(&mut dma, 0x2F00, 0x20_0000, &data),
            Err(Status::DATA_TRANSFER_ERROR)
        );
    }

    #[test]
    fn read_from_host_gathers_pages_and_reports_unmapped_sources() {
        let mut dma = MockDma::new(0x1_0000);
        let data: Vec<u8> = (0..0x300).map(|index| index as u8).collect();
        write_to_host(&mut dma, 0x2F00, 0x9000, &data).unwrap();
        assert_eq!(read_from_host(&mut dma, 0x2F00, 0x9000, 0x300), Ok(data));
        assert_eq!(
            read_from_host(&mut dma, 0x2F00, 0x20_0000, 0x300),
            Err(Status::DATA_TRANSFER_ERROR)
        );
    }

    #[test]
    fn cqe_carries_phase_status_and_identifiers() {
        let raw = encode_cqe(
            Completion {
                dw0: 0xAABB_CCDD,
                status: Status::INVALID_QUEUE_SIZE,
            },
            5,
            1,
            0x1234,
            true,
        );
        assert_eq!(&raw[0..4], &0xAABB_CCDDu32.to_le_bytes());
        assert_eq!(&raw[4..8], &[0; 4]);
        assert_eq!(
            u32::from_le_bytes(raw[8..12].try_into().unwrap()),
            5 | (1 << 16)
        );
        let dw3 = u32::from_le_bytes(raw[12..16].try_into().unwrap());
        assert_eq!(dw3 & 0xFFFF, 0x1234);
        assert_eq!((dw3 >> 16) & 1, 1, "phase tag");
        assert_eq!((dw3 >> 17) & 0xFF, 0x02, "SC");
        assert_eq!((dw3 >> 25) & 0x7, 1, "SCT");
        assert_eq!(dw3 >> 31, 1, "DNR");
    }

    #[test]
    fn completion_queue_flips_the_phase_on_wrap_and_reports_full() {
        let mut cq = CompletionQueue::new(0x1000, 4, false);
        assert!(cq.phase);
        for _ in 0..3 {
            assert!(!cq.is_full());
            cq.advance_tail();
        }
        assert!(cq.is_full());
        cq.head = 2;
        assert!(!cq.is_full());
        cq.advance_tail();
        assert_eq!(cq.tail, 0);
        assert!(!cq.phase);
    }

    #[test]
    fn command_decode_reads_the_documented_dword_positions() {
        let mut raw = [0u8; SQE_BYTES];
        raw[0..4].copy_from_slice(&(0x06u32 | (0x1234 << 16) | (1 << 14)).to_le_bytes());
        raw[4..8].copy_from_slice(&7u32.to_le_bytes());
        raw[24..32].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        raw[32..40].copy_from_slice(&0x99u64.to_le_bytes());
        raw[40..44].copy_from_slice(&0xA0u32.to_le_bytes());
        raw[52..56].copy_from_slice(&0xD0u32.to_le_bytes());
        let command = Command::decode(&raw);
        assert_eq!(command.opcode, 6);
        assert_eq!(command.cid, 0x1234);
        assert_eq!(command.psdt, 1);
        assert_eq!(command.nsid, 7);
        assert_eq!(command.prp1, 0x1122_3344_5566_7788);
        assert_eq!(command.prp2, 0x99);
        assert_eq!(command.cdw10, 0xA0);
        assert_eq!(command.cdw13, 0xD0);
    }
}
