//! Tests of the NVMe controller through `MachineBus`, the way a driver uses it:
//! registers, doorbells, command and completion queues in guest RAM.

use super::controller::{MAX_IO_QUEUES, VERSION};
use super::queues::{CQE_BYTES, SQE_BYTES};
use super::*;
use crate::{
    AccessWidth, BusAccessError, DeviceError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
};

const RAM_BYTES: u64 = 16 << 20;
const ASQ: u64 = 0x1_0000;
const ACQ: u64 = 0x2_0000;
const DATA: u64 = 0x3_0000;
const IO_SQ: u64 = 0x4_0000;
const IO_CQ: u64 = 0x5_0000;
const ADMIN_ENTRIES: u16 = 16;
const SECTORS: u64 = 0x0010_0000;

const ADMIN_SQ_DOORBELL: u64 = 0x1000;
const ADMIN_CQ_DOORBELL: u64 = 0x1004;
const IO1_SQ_DOORBELL: u64 = 0x1008;
const IO1_CQ_DOORBELL: u64 = 0x100C;

const OPC_DELETE_SQ: u8 = 0x00;
const OPC_CREATE_SQ: u8 = 0x01;
const OPC_GET_LOG_PAGE: u8 = 0x02;
const OPC_DELETE_CQ: u8 = 0x04;
const OPC_CREATE_CQ: u8 = 0x05;
const OPC_IDENTIFY: u8 = 0x06;
const OPC_ABORT: u8 = 0x08;
const OPC_SET_FEATURES: u8 = 0x09;
const OPC_GET_FEATURES: u8 = 0x0A;
const OPC_AER: u8 = 0x0C;

fn config() -> NvmeConfig {
    NvmeConfig {
        namespace_sectors: SECTORS,
        ..NvmeConfig::default()
    }
}

/// A submission queue entry under construction.
#[derive(Clone, Copy, Default)]
struct Sqe {
    opcode: u8,
    psdt: u8,
    nsid: u32,
    prp1: u64,
    prp2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
}

impl Sqe {
    fn new(opcode: u8) -> Self {
        Self {
            opcode,
            ..Self::default()
        }
    }
    fn nsid(mut self, nsid: u32) -> Self {
        self.nsid = nsid;
        self
    }
    fn prp(mut self, prp1: u64, prp2: u64) -> Self {
        self.prp1 = prp1;
        self.prp2 = prp2;
        self
    }
    fn cdw(mut self, cdw10: u32, cdw11: u32) -> Self {
        self.cdw10 = cdw10;
        self.cdw11 = cdw11;
        self
    }
    fn with_offset(mut self, offset: u32) -> Self {
        self.cdw12 = offset;
        self
    }
    fn raw(&self, cid: u16) -> [u8; SQE_BYTES] {
        let mut raw = [0u8; SQE_BYTES];
        let dw0 = u32::from(self.opcode) | (u32::from(self.psdt) << 14) | (u32::from(cid) << 16);
        raw[0..4].copy_from_slice(&dw0.to_le_bytes());
        raw[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        raw[24..32].copy_from_slice(&self.prp1.to_le_bytes());
        raw[32..40].copy_from_slice(&self.prp2.to_le_bytes());
        raw[40..44].copy_from_slice(&self.cdw10.to_le_bytes());
        raw[44..48].copy_from_slice(&self.cdw11.to_le_bytes());
        raw[48..52].copy_from_slice(&self.cdw12.to_le_bytes());
        raw[52..56].copy_from_slice(&self.cdw13.to_le_bytes());
        raw
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cqe {
    dw0: u32,
    sqhd: u16,
    sqid: u16,
    cid: u16,
    phase: bool,
    sc: u8,
    sct: u8,
    dnr: bool,
}

impl Cqe {
    fn parse(raw: &[u8; CQE_BYTES]) -> Self {
        let dw =
            |index: usize| u32::from_le_bytes(raw[index * 4..index * 4 + 4].try_into().unwrap());
        Self {
            dw0: dw(0),
            sqhd: dw(2) as u16,
            sqid: (dw(2) >> 16) as u16,
            cid: dw(3) as u16,
            phase: (dw(3) >> 16) & 1 == 1,
            sc: ((dw(3) >> 17) & 0xFF) as u8,
            sct: ((dw(3) >> 25) & 0x7) as u8,
            dnr: dw(3) >> 31 == 1,
        }
    }
    fn status(&self) -> (u8, u8) {
        (self.sct, self.sc)
    }
}

struct Rig {
    bus: MachineBus,
    nvme: NvmeHandle,
    sq_tail: u16,
    cq_head: u16,
    cq_phase: bool,
    next_cid: u16,
}

fn rig_with(config: NvmeConfig) -> Rig {
    let (device, nvme) = NvmeDevice::new_with_handle(config).unwrap();
    let range =
        |start, len| GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_BYTES), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(NVME_MMIO_BASE, NVME_MMIO_BYTES),
                MemoryRegionKind::Mmio,
            ),
        ],
    )
    .unwrap();
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_BYTES),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(NVME_MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    Rig {
        bus,
        nvme,
        sq_tail: 0,
        cq_head: 0,
        cq_phase: true,
        next_cid: 0x100,
    }
}

fn rig() -> Rig {
    rig_with(config())
}

const CC_ENABLE: u32 = 1 | (6 << 16) | (4 << 20);

impl Rig {
    fn address(offset: u64) -> GuestPhysicalAddress {
        GuestPhysicalAddress::new(NVME_MMIO_BASE + offset)
    }
    fn read(&mut self, offset: u64, width: AccessWidth) -> Result<u64, BusAccessError> {
        self.bus.read_physical(Self::address(offset), width)
    }
    fn write(&mut self, offset: u64, width: AccessWidth, value: u64) -> Result<(), BusAccessError> {
        self.bus
            .write_physical(Self::address(offset), width, value, |_| {})
    }
    fn dword(&mut self, offset: u64) -> u32 {
        self.read(offset, AccessWidth::Dword).unwrap() as u32
    }
    fn set_dword(&mut self, offset: u64, value: u32) {
        self.write(offset, AccessWidth::Dword, u64::from(value))
            .unwrap();
    }
    fn mem_write(&mut self, address: u64, bytes: &[u8]) {
        self.bus
            .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
            .unwrap();
    }
    fn mem_read(&mut self, address: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        self.bus
            .read_flat_bytes(GuestPhysicalAddress::new(address), &mut bytes)
            .unwrap();
        bytes
    }
    fn service(&mut self) -> NvmeServiceReport {
        self.nvme.service(&mut self.bus, |_| {})
    }

    /// Programs AQA/ASQ/ACQ and enables the controller.
    fn enable(&mut self) {
        let aqa = u32::from(ADMIN_ENTRIES - 1) | (u32::from(ADMIN_ENTRIES - 1) << 16);
        self.set_dword(0x24, aqa);
        self.write(0x28, AccessWidth::Qword, ASQ).unwrap();
        self.write(0x30, AccessWidth::Qword, ACQ).unwrap();
        self.set_dword(0x14, CC_ENABLE);
        assert_eq!(self.dword(0x1C) & 0b11, 1, "RDY set, CFS clear");
    }

    fn enabled() -> Self {
        let mut rig = rig();
        rig.enable();
        rig
    }

    fn poll_admin(&mut self) -> Option<Cqe> {
        let address = ACQ + u64::from(self.cq_head) * CQE_BYTES as u64;
        let raw: [u8; CQE_BYTES] = self.mem_read(address, CQE_BYTES).try_into().unwrap();
        let cqe = Cqe::parse(&raw);
        if cqe.phase != self.cq_phase {
            return None;
        }
        self.cq_head += 1;
        if self.cq_head == ADMIN_ENTRIES {
            self.cq_head = 0;
            self.cq_phase = !self.cq_phase;
        }
        self.set_dword(ADMIN_CQ_DOORBELL, u32::from(self.cq_head));
        Some(cqe)
    }

    /// Submits one admin command and returns its completion, if one was posted.
    fn admin_try(&mut self, sqe: Sqe) -> (u16, Option<Cqe>) {
        let cid = self.next_cid;
        self.next_cid += 1;
        let address = ASQ + u64::from(self.sq_tail) * SQE_BYTES as u64;
        self.mem_write(address, &sqe.raw(cid));
        self.sq_tail = (self.sq_tail + 1) % ADMIN_ENTRIES;
        self.set_dword(ADMIN_SQ_DOORBELL, u32::from(self.sq_tail));
        assert!(self.nvme.doorbell_pending());
        self.service();
        assert!(!self.nvme.doorbell_pending());
        (cid, self.poll_admin())
    }

    fn admin(&mut self, sqe: Sqe) -> Cqe {
        let (cid, cqe) = self.admin_try(sqe);
        let cqe = cqe.expect("admin command completed");
        assert_eq!(cqe.cid, cid);
        assert_eq!(cqe.sqid, 0);
        cqe
    }

    fn identify(&mut self, cns: u32, nsid: u32) -> (Cqe, Vec<u8>) {
        self.mem_write(DATA, &[0xAA; 4096]);
        let cqe = self.admin(Sqe::new(OPC_IDENTIFY).nsid(nsid).prp(DATA, 0).cdw(cns, 0));
        (cqe, self.mem_read(DATA, 4096))
    }

    fn log_page(&mut self, lid: u8, bytes: usize, nsid: u32, rae: bool) -> (Cqe, Vec<u8>) {
        self.mem_write(DATA, &vec![0xAA; bytes]);
        let numd = (bytes / 4 - 1) as u32;
        let cdw10 = u32::from(lid) | (u32::from(rae) << 15) | (numd << 16);
        let cqe = self.admin(
            Sqe::new(OPC_GET_LOG_PAGE)
                .nsid(nsid)
                .prp(DATA, 0)
                .cdw(cdw10, 0),
        );
        (cqe, self.mem_read(DATA, bytes))
    }

    fn create_io_queues(&mut self) {
        let cq = self.admin(Sqe::new(OPC_CREATE_CQ).prp(IO_CQ, 0).cdw(1 | (3 << 16), 1));
        assert_eq!(cq.status(), (0, 0));
        let sq = self.admin(
            Sqe::new(OPC_CREATE_SQ)
                .prp(IO_SQ, 0)
                .cdw(1 | (3 << 16), 1 | (1 << 16)),
        );
        assert_eq!(sq.status(), (0, 0));
    }

    fn device_error(result: Result<u64, BusAccessError>) -> DeviceError {
        match result {
            Err(BusAccessError::Device { source, .. }) => source,
            other => panic!("expected a device error, got {other:?}"),
        }
    }
    fn write_error(result: Result<(), BusAccessError>) -> DeviceError {
        match result {
            Err(BusAccessError::Device { source, .. }) => source,
            other => panic!("expected a device error, got {other:?}"),
        }
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// ---------------------------------------------------------------- registers

#[test]
fn cap_and_vs_report_the_declared_capabilities() {
    let mut rig = rig();
    let cap = rig.read(0x00, AccessWidth::Qword).unwrap();
    assert_eq!(cap & 0xFFFF, 1023, "MQES, 0's based");
    assert_eq!((cap >> 16) & 1, 1, "CQR: contiguous queues required");
    assert_eq!((cap >> 17) & 3, 0, "AMS: round robin only");
    assert_eq!((cap >> 24) & 0xFF, 1, "TO");
    assert_eq!((cap >> 32) & 0xF, 0, "DSTRD");
    assert_eq!((cap >> 36) & 1, 0, "NSSRS");
    assert_eq!((cap >> 37) & 0xFF, 1, "CSS: NVM command set only");
    assert_eq!((cap >> 48) & 0xFF, 0, "MPSMIN = MPSMAX = 4 KiB");
    assert_eq!(cap >> 56, 0, "no PMR, CMB, NSSS, CRMS");
    // The two dword halves read the same value.
    assert_eq!(u64::from(rig.dword(0x00)), cap & 0xFFFF_FFFF);
    assert_eq!(u64::from(rig.dword(0x04)), cap >> 32);
    assert_eq!(rig.dword(0x08), VERSION);
    assert_eq!(rig.dword(0x14), 0, "CC resets to 0");
    assert_eq!(rig.dword(0x1C), 0, "CSTS resets to 0");
}

#[test]
fn register_access_errors_are_device_errors() {
    let mut rig = rig();
    assert_eq!(
        Rig::device_error(rig.read(0x08, AccessWidth::Byte)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.read(0x0A, AccessWidth::Word)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.read(0x02, AccessWidth::Dword)),
        DeviceError::Misaligned
    );
    assert_eq!(
        Rig::device_error(rig.read(0x04, AccessWidth::Qword)),
        DeviceError::Misaligned
    );
    // A qword at 0x08 spans VS and INTMS.
    assert_eq!(
        Rig::device_error(rig.read(0x08, AccessWidth::Qword)),
        DeviceError::CrossesRegister
    );
    for offset in [0x00, 0x08, 0x1C] {
        assert_eq!(
            Rig::write_error(rig.write(offset, AccessWidth::Dword, 1)),
            DeviceError::ReadOnly,
            "offset {offset:#x}"
        );
    }
    // Reserved properties read as zero and ignore writes.
    rig.set_dword(0x100, 0xFFFF_FFFF);
    assert_eq!(rig.dword(0x100), 0);
    rig.set_dword(0x20, 0x4E56_4D65);
    assert_eq!(rig.dword(0x20), 0, "NSSR is not implemented");
}

#[test]
fn intms_and_intmc_set_and_clear_the_same_mask() {
    let mut rig = rig();
    rig.set_dword(0x0C, 0b1010);
    assert_eq!(rig.dword(0x0C), 0b1010);
    assert_eq!(rig.dword(0x10), 0b1010);
    rig.set_dword(0x10, 0b0010);
    assert_eq!(rig.dword(0x0C), 0b1000);
}

#[test]
fn enable_sets_rdy_and_disable_clears_it_and_the_queues() {
    let mut rig = Rig::enabled();
    assert_eq!(rig.dword(0x14) & 1, 1);
    rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0));
    // CC.EN = 0 is a controller reset.
    rig.set_dword(0x14, 0);
    assert_eq!(rig.dword(0x1C), 0, "RDY cleared");
    assert!(!rig.nvme.is_ready());
    assert_eq!(
        Rig::write_error(rig.write(ADMIN_SQ_DOORBELL, AccessWidth::Dword, 1)),
        DeviceError::UnsupportedConfiguration,
        "doorbells are dead while RDY = 0"
    );
    // After a reset the admin queues are created afresh and start empty.
    let mut again = Rig::enabled();
    again.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0));
    again.set_dword(0x14, 0);
    again.sq_tail = 0;
    again.cq_head = 0;
    again.cq_phase = true;
    again.mem_write(ACQ, &[0u8; CQE_BYTES * 2]);
    again.enable();
    let cqe = again.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0));
    assert_eq!(cqe.sqhd, 1);
}

#[test]
fn invalid_cc_values_set_cfs_instead_of_rdy() {
    for (name, cc) in [
        ("MPS = 1", CC_ENABLE | (1 << 7)),
        ("CSS = 1", CC_ENABLE | (1 << 4)),
        ("AMS = 1", CC_ENABLE | (1 << 11)),
        ("IOSQES = 5", (CC_ENABLE & !(0xF << 16)) | (5 << 16)),
        ("IOCQES = 3", (CC_ENABLE & !(0xF << 20)) | (3 << 20)),
    ] {
        let mut rig = rig();
        rig.set_dword(0x24, 0x000F_000F);
        rig.write(0x28, AccessWidth::Qword, ASQ).unwrap();
        rig.write(0x30, AccessWidth::Qword, ACQ).unwrap();
        rig.set_dword(0x14, cc);
        assert_eq!(rig.dword(0x1C), 0b10, "{name}: CFS set, RDY clear");
        assert!(!rig.nvme.is_ready());
        rig.set_dword(0x14, 0);
        assert_eq!(rig.dword(0x1C), 0, "{name}: reset clears CFS");
    }
    // AQA with a one-entry queue is invalid, too.
    let mut rig = rig();
    rig.set_dword(0x24, 0x0000_0000);
    rig.set_dword(0x14, CC_ENABLE);
    assert_eq!(rig.dword(0x1C), 0b10);
}

#[test]
fn admin_queue_registers_are_busy_while_enabled() {
    let mut rig = Rig::enabled();
    assert_eq!(
        Rig::write_error(rig.write(0x24, AccessWidth::Dword, 1)),
        DeviceError::Busy
    );
    assert_eq!(
        Rig::write_error(rig.write(0x28, AccessWidth::Qword, 0x9000)),
        DeviceError::Busy
    );
    assert_eq!(rig.read(0x28, AccessWidth::Qword).unwrap(), ASQ);
}

#[test]
fn asq_and_acq_accept_dword_halves_and_ignore_the_low_12_bits() {
    let mut rig = rig();
    rig.set_dword(0x28, 0x1234_5FFF);
    rig.set_dword(0x2C, 0x0000_0001);
    assert_eq!(rig.read(0x28, AccessWidth::Qword).unwrap(), 0x1_1234_5000);
}

#[test]
fn shutdown_notification_completes_immediately() {
    let mut rig = Rig::enabled();
    rig.set_dword(0x14, CC_ENABLE | (1 << 14));
    assert_eq!((rig.dword(0x1C) >> 2) & 3, 0b10, "SHST: shutdown complete");
    assert_eq!(rig.dword(0x1C) & 1, 1, "RDY stays set until CC.EN = 0");
}

// ---------------------------------------------------------------- doorbells

#[test]
fn doorbell_outside_the_declared_queues_is_an_error() {
    let mut rig = Rig::enabled();
    let last = 0x1000 + 8 * MAX_IO_QUEUES as u64 + 4;
    // The CQ doorbell of the last offered queue is in range (its queue is just not created).
    assert!(rig.write(last, AccessWidth::Dword, 0).is_ok());
    for offset in [last + 4, 0x1100, 0x1FFC] {
        assert_eq!(
            Rig::write_error(rig.write(offset, AccessWidth::Dword, 1)),
            DeviceError::UnknownOffset,
            "offset {offset:#x}"
        );
        assert_eq!(
            Rig::device_error(rig.read(offset, AccessWidth::Dword)),
            DeviceError::UnknownOffset
        );
    }
    assert_eq!(
        Rig::write_error(rig.write(0x1001, AccessWidth::Dword, 1)),
        DeviceError::Misaligned
    );
    assert_eq!(
        Rig::write_error(rig.write(0x1000, AccessWidth::Word, 1)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.read(0x1000, AccessWidth::Dword)),
        DeviceError::WriteOnly
    );
}

#[test]
fn doorbell_value_not_below_the_queue_size_is_logged_and_ignored() {
    let mut rig = Rig::enabled();
    // The device accepts the write (value out of range) but changes nothing.
    rig.set_dword(ADMIN_SQ_DOORBELL, u32::from(ADMIN_ENTRIES));
    assert!(!rig.nvme.doorbell_pending());
    assert_eq!(rig.service().commands, 0);
    let (cqe, page) = rig.log_page(0x01, 4096, 0xFFFF_FFFF, true);
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(u64_at(&page, 0), 1, "error count");
    assert_eq!(u16_at(&page, 8), 0, "SQID");
    assert_eq!(u16_at(&page, 10), 0xFFFF, "no command id");
    assert_eq!(u32_at(&page, 24), 0xFFFF_FFFF, "no namespace");
}

#[test]
fn doorbell_of_a_queue_that_does_not_exist_raises_the_invalid_register_event() {
    let mut rig = Rig::enabled();
    let (aer_cid, none) = rig.admin_try(Sqe::new(OPC_AER));
    assert!(none.is_none(), "AER stays outstanding");
    rig.set_dword(IO1_SQ_DOORBELL, 1);
    rig.service();
    let event = rig.poll_admin().expect("event completion");
    assert_eq!(event.cid, aer_cid);
    // Event type 0 (error), info 0 (invalid doorbell register), log page 1.
    assert_eq!(event.dw0, 0x0001_0000);
    assert_eq!(event.status(), (0, 0));
}

// ---------------------------------------------------------------- identify

#[test]
fn identify_controller_matches_the_declared_structure_byte_for_byte() {
    let mut rig = Rig::enabled();
    let (cqe, page) = rig.identify(1, 0);
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(cqe.sqhd, 1);

    let mut expected = vec![0u8; 4096];
    let put = |expected: &mut Vec<u8>, offset: usize, bytes: &[u8]| {
        expected[offset..offset + bytes.len()].copy_from_slice(bytes);
    };
    put(&mut expected, 4, b"OD-NVME-0001        "); // SN
    put(
        &mut expected,
        24,
        b"Open Desktop Virtual NVMe               ",
    ); // MN
    put(&mut expected, 64, b"0.1.0   "); // FR
    expected[77] = 8; // MDTS: 2^8 pages = 1 MiB
    put(&mut expected, 80, &0x0002_0000u32.to_le_bytes()); // VER
    expected[111] = 1; // CNTRLTYPE: I/O controller
    expected[258] = 3; // ACL
    expected[259] = 3; // AERL
    expected[260] = 0x03; // FRMW
    expected[262] = 63; // ELPE
    put(&mut expected, 266, &343u16.to_le_bytes()); // WCTEMP
    put(&mut expected, 268, &358u16.to_le_bytes()); // CCTEMP
    put(
        &mut expected,
        280,
        &(u128::from(SECTORS) * 512).to_le_bytes(),
    ); // TNVMCAP
    expected[512] = 0x66; // SQES
    expected[513] = 0x44; // CQES
    put(&mut expected, 516, &1u32.to_le_bytes()); // NN
    let nqn = b"nqn.2014-08.org.nvmexpress:uuid:4f70656e-4465-736b-746f-704e564d6553";
    put(&mut expected, 768, nqn); // SUBNQN
    put(&mut expected, 2048, &2500u16.to_le_bytes()); // PSD0.MP
    assert_eq!(page, expected);
}

#[test]
fn identify_controller_declares_no_unimplemented_capability() {
    let mut rig = Rig::enabled();
    let (_, page) = rig.identify(1, 0);
    assert_eq!(
        u16_at(&page, 256),
        0,
        "OACS: no security/format/firmware/NS management"
    );
    assert_eq!(u16_at(&page, 520), 0, "ONCS: only Read, Write, Flush");
    assert_eq!(u16_at(&page, 522), 0, "FUSES: no fused operations");
    assert_eq!(u32_at(&page, 536), 0, "SGLS: SGL not supported");
    assert_eq!(page[525], 0, "VWC: no volatile write cache");
    assert_eq!(
        u32_at(&page, 92),
        0,
        "OAES: no optional asynchronous events"
    );
    assert_eq!(page[263], 0, "NPSS: one power state");
}

#[test]
fn identify_namespace_matches_the_declared_structure_byte_for_byte() {
    let mut rig = Rig::enabled();
    let (cqe, page) = rig.identify(0, 1);
    assert_eq!(cqe.status(), (0, 0));
    let mut expected = vec![0u8; 4096];
    let sectors = SECTORS.to_le_bytes();
    expected[0..8].copy_from_slice(&sectors); // NSZE
    expected[8..16].copy_from_slice(&sectors); // NCAP
    expected[16..24].copy_from_slice(&sectors); // NUSE
    expected[48..64].copy_from_slice(&(u128::from(SECTORS) * 512).to_le_bytes()); // NVMCAP
    expected[104..120].copy_from_slice(b"OpenDesktopNVMeN"); // NGUID
    expected[128..132].copy_from_slice(&0x0009_0000u32.to_le_bytes()); // LBAF0: LBADS = 9
    assert_eq!(page, expected);
    assert_eq!(page[25], 0, "NLBAF: one LBA format");
    assert_eq!(page[26] & 0xF, 0, "FLBAS: format 0 in use");
}

#[test]
fn identify_namespace_rejects_other_namespace_ids() {
    let mut rig = Rig::enabled();
    for nsid in [0, 2, 0xFFFF_FFFE, 0xFFFF_FFFF] {
        let (cqe, _) = rig.identify(0, nsid);
        assert_eq!(
            cqe.status(),
            (0, 0x0B),
            "nsid {nsid:#x}: Invalid Namespace or Format"
        );
        assert!(cqe.dnr);
    }
}

#[test]
fn active_namespace_list_holds_namespace_one() {
    let mut rig = Rig::enabled();
    let (cqe, list) = rig.identify(2, 0);
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(u32_at(&list, 0), 1);
    assert!(
        list[4..].iter().all(|&b| b == 0),
        "list ends with a zero entry"
    );
    // The list holds NSIDs greater than the one in the command.
    let (_, list) = rig.identify(2, 1);
    assert!(list.iter().all(|&b| b == 0));
    let (cqe, _) = rig.identify(2, 0xFFFF_FFFE);
    assert_eq!(cqe.status(), (0, 0x0B));
}

#[test]
fn namespace_descriptor_list_has_nguid_and_command_set() {
    let mut rig = Rig::enabled();
    let (cqe, page) = rig.identify(3, 1);
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(&page[0..4], &[2, 16, 0, 0]);
    assert_eq!(&page[4..20], b"OpenDesktopNVMeN");
    assert_eq!(&page[20..24], &[4, 1, 0, 0]);
    assert_eq!(page[24], 0, "CSI 0 = NVM command set");
    assert!(
        page[25..].iter().all(|&b| b == 0),
        "terminating zero descriptor"
    );
}

#[test]
fn identify_with_unsupported_cns_or_sgl_is_invalid_field() {
    let mut rig = Rig::enabled();
    let (cqe, _) = rig.identify(0x77, 0);
    assert_eq!(cqe.status(), (0, 0x02));
    let mut sqe = Sqe::new(OPC_IDENTIFY).prp(DATA, 0).cdw(1, 0);
    sqe.psdt = 1;
    assert_eq!(rig.admin(sqe).status(), (0, 0x02), "SGL is not supported");
}

#[test]
fn identify_with_a_prp_outside_ram_is_a_data_transfer_error() {
    let mut rig = Rig::enabled();
    let cqe = rig.admin(Sqe::new(OPC_IDENTIFY).prp(RAM_BYTES + 0x1000, 0).cdw(1, 0));
    assert_eq!(cqe.status(), (0, 0x04));
    assert!(!cqe.dnr, "a transfer error may be retried");
    // PRP1 inside the NVMe window itself is not RAM either: no re-entrancy.
    let cqe = rig.admin(Sqe::new(OPC_IDENTIFY).prp(NVME_MMIO_BASE, 0).cdw(1, 0));
    assert_eq!(cqe.status(), (0, 0x04));
}

#[test]
fn identify_transfer_crosses_a_page_boundary_with_prp2() {
    let mut rig = Rig::enabled();
    let (_, whole) = rig.identify(1, 0);
    // PRP1 starts 0x800 into a page: 0x800 bytes there, the rest at PRP2.
    let prp1 = DATA + 0x1800;
    let prp2 = DATA + 0x4000;
    rig.mem_write(prp1, &[0xAA; 0x800]);
    rig.mem_write(prp2, &[0xAA; 0x1000]);
    let cqe = rig.admin(Sqe::new(OPC_IDENTIFY).prp(prp1, prp2).cdw(1, 0));
    assert_eq!(cqe.status(), (0, 0));
    let mut joined = rig.mem_read(prp1, 0x800);
    joined.extend(rig.mem_read(prp2, 0x800));
    assert_eq!(joined, whole[..0x1000].to_vec());
    // Nothing beyond the 4096 bytes is touched.
    assert_eq!(rig.mem_read(prp2 + 0x800, 0x800), vec![0xAA; 0x800]);
    // A PRP2 that is not page aligned is PRP Offset Invalid.
    let cqe = rig.admin(Sqe::new(OPC_IDENTIFY).prp(prp1, prp2 + 8).cdw(1, 0));
    assert_eq!(cqe.status(), (0, 0x13));
}

// ---------------------------------------------------------------- queues

#[test]
fn io_queues_are_created_and_completions_carry_the_phase_bit() {
    let mut rig = Rig::enabled();
    rig.create_io_queues();
    // Slice A has no I/O command set: every I/O command is an invalid opcode,
    // but it still completes, in order, with a flipping phase tag.
    let mut head = 0u16;
    let mut phase = true;
    let mut tail = 0u16;
    for round in 0..5u16 {
        let cid = 0x700 + round;
        let sqe = Sqe::new(0x02).nsid(1);
        rig.mem_write(IO_SQ + u64::from(tail) * SQE_BYTES as u64, &sqe.raw(cid));
        tail = (tail + 1) % 4;
        rig.set_dword(IO1_SQ_DOORBELL, u32::from(tail));
        rig.service();
        let raw = rig.mem_read(IO_CQ + u64::from(head) * CQE_BYTES as u64, CQE_BYTES);
        let cqe = Cqe::parse(&raw.try_into().unwrap());
        assert_eq!(cqe.cid, cid);
        assert_eq!(cqe.sqid, 1);
        assert_eq!(cqe.sqhd, tail);
        assert_eq!(cqe.phase, phase, "round {round}");
        assert_eq!(cqe.status(), (0, 0x01), "Invalid Command Opcode");
        head = (head + 1) % 4;
        if head == 0 {
            phase = !phase;
        }
        rig.set_dword(IO1_CQ_DOORBELL, u32::from(head));
    }
    assert!(!phase, "the phase flipped once after the fourth entry");
}

#[test]
fn full_completion_queue_stops_servicing_until_the_host_moves_the_head() {
    let mut rig = Rig::enabled();
    rig.create_io_queues();
    // A 4-entry CQ holds 3 completions (head == tail means empty).
    for index in 0..3u16 {
        rig.mem_write(
            IO_SQ + u64::from(index) * SQE_BYTES as u64,
            &Sqe::new(0x02).nsid(1).raw(0x800 + index),
        );
    }
    rig.set_dword(IO1_SQ_DOORBELL, 3);
    assert_eq!(rig.service().completions, 3);
    rig.mem_write(
        IO_SQ + 3 * SQE_BYTES as u64,
        &Sqe::new(0x02).nsid(1).raw(0x803),
    );
    rig.set_dword(IO1_SQ_DOORBELL, 0);
    let report = rig.service();
    assert_eq!(
        (report.commands, report.completions),
        (0, 0),
        "CQ full: command stays queued"
    );
    // The host consumes entries; the command is then executed and completes in slot 3.
    rig.set_dword(IO1_CQ_DOORBELL, 3);
    assert!(rig.nvme.doorbell_pending());
    let report = rig.service();
    assert_eq!((report.commands, report.completions), (1, 1));
    let raw = rig.mem_read(IO_CQ + 3 * CQE_BYTES as u64, CQE_BYTES);
    let cqe = Cqe::parse(&raw.try_into().unwrap());
    assert_eq!((cqe.cid, cqe.sqhd, cqe.phase), (0x803, 0, true));
    // Entries 0..2 were never overwritten.
    let raw = rig.mem_read(IO_CQ, CQE_BYTES);
    assert_eq!(Cqe::parse(&raw.try_into().unwrap()).cid, 0x800);
}

#[test]
fn create_sq_without_its_cq_is_completion_queue_invalid() {
    let mut rig = Rig::enabled();
    let cqe = rig.admin(
        Sqe::new(OPC_CREATE_SQ)
            .prp(IO_SQ, 0)
            .cdw(1 | (3 << 16), 1 | (1 << 16)),
    );
    assert_eq!(cqe.status(), (1, 0x00));
    // Once the CQ exists, creation succeeds; the same ids are then taken.
    let cq = rig.admin(Sqe::new(OPC_CREATE_CQ).prp(IO_CQ, 0).cdw(1 | (3 << 16), 1));
    assert_eq!(cq.status(), (0, 0));
    let sq = rig.admin(
        Sqe::new(OPC_CREATE_SQ)
            .prp(IO_SQ, 0)
            .cdw(1 | (3 << 16), 1 | (1 << 16)),
    );
    assert_eq!(sq.status(), (0, 0));
    let again = rig.admin(Sqe::new(OPC_CREATE_CQ).prp(IO_CQ, 0).cdw(1 | (3 << 16), 1));
    assert_eq!(again.status(), (1, 0x01), "Invalid Queue Identifier");
}

#[test]
fn queue_creation_validates_identifier_size_and_contiguity() {
    let mut rig = Rig::enabled();
    let create_cq = |cdw10, cdw11, prp1| Sqe::new(OPC_CREATE_CQ).prp(prp1, 0).cdw(cdw10, cdw11);
    // Queue id 0 and ids above the offered count.
    assert_eq!(rig.admin(create_cq(3 << 16, 1, IO_CQ)).status(), (1, 0x01));
    assert_eq!(
        rig.admin(create_cq(5 | (3 << 16), 1, IO_CQ)).status(),
        (1, 0x01)
    );
    // Size: QSIZE 0 means one entry (below the minimum of two); 1024 is the maximum.
    assert_eq!(rig.admin(create_cq(1, 1, IO_CQ)).status(), (1, 0x02));
    assert_eq!(
        rig.admin(create_cq(1 | (1024 << 16), 1, IO_CQ)).status(),
        (1, 0x02)
    );
    // Not physically contiguous (PC = 0) is refused because CAP.CQR = 1.
    assert_eq!(
        rig.admin(create_cq(1 | (3 << 16), 0, IO_CQ)).status(),
        (0, 0x02)
    );
    // PRP1 must be page aligned.
    assert_eq!(
        rig.admin(create_cq(1 | (3 << 16), 1, IO_CQ + 0x100))
            .status(),
        (0, 0x02)
    );
    // Interrupt vectors other than 0 do not exist with pin-based interrupts.
    assert_eq!(
        rig.admin(create_cq(1 | (3 << 16), 1 | 0b10 | (1 << 16), IO_CQ))
            .status(),
        (1, 0x08)
    );
    // The largest legal queue is accepted.
    assert_eq!(
        rig.admin(create_cq(1 | (1023 << 16), 1, IO_CQ)).status(),
        (0, 0)
    );
}

#[test]
fn deleting_a_cq_that_a_sq_uses_is_invalid_queue_deletion() {
    let mut rig = Rig::enabled();
    rig.create_io_queues();
    let delete_cq = Sqe::new(OPC_DELETE_CQ).cdw(1, 0);
    assert_eq!(rig.admin(delete_cq).status(), (1, 0x0C));
    assert_eq!(
        rig.admin(Sqe::new(OPC_DELETE_SQ).cdw(1, 0)).status(),
        (0, 0)
    );
    assert_eq!(rig.admin(delete_cq).status(), (0, 0));
    // Deleted queues are gone: ids are invalid again, and can be reused.
    assert_eq!(
        rig.admin(Sqe::new(OPC_DELETE_SQ).cdw(1, 0)).status(),
        (1, 0x01)
    );
    assert_eq!(
        rig.admin(Sqe::new(OPC_DELETE_CQ).cdw(1, 0)).status(),
        (1, 0x01)
    );
    assert_eq!(
        rig.admin(Sqe::new(OPC_DELETE_CQ).cdw(0, 0)).status(),
        (1, 0x01),
        "admin CQ"
    );
    rig.create_io_queues();
}

// ---------------------------------------------------------------- features

#[test]
fn number_of_queues_is_reported_and_can_be_lowered_before_queues_exist() {
    let mut rig = Rig::enabled();
    let get = Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0);
    let cqe = rig.admin(get);
    assert_eq!(cqe.status(), (0, 0));
    // NSQA and NCQA, 0's based: four I/O queue pairs.
    assert_eq!(cqe.dw0, 3 | (3 << 16));
    // Ask for more than offered: the controller grants what it has.
    let set = rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x07, 63 | (63 << 16)));
    assert_eq!((set.status(), set.dw0), ((0, 0), 3 | (3 << 16)));
    let set = rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x07, 1 | (2 << 16)));
    assert_eq!((set.status(), set.dw0), ((0, 0), 1 | (2 << 16)));
    assert_eq!(rig.admin(get).dw0, 1 | (2 << 16));
    // Queue ids above the granted count are refused.
    let cq3 = Sqe::new(OPC_CREATE_CQ).prp(IO_CQ, 0).cdw(4 | (3 << 16), 1);
    assert_eq!(rig.admin(cq3).status(), (1, 0x01));
    let cq3 = Sqe::new(OPC_CREATE_CQ).prp(IO_CQ, 0).cdw(3 | (3 << 16), 1);
    assert_eq!(rig.admin(cq3).status(), (0, 0));
    // Once an I/O queue exists the number cannot change.
    let late = rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x07, 0));
    assert_eq!(late.status(), (0, 0x0C), "Command Sequence Error");
    // 0xFFFF is reserved.
    let mut fresh = Rig::enabled();
    let bad = fresh.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x07, 0xFFFF));
    assert_eq!(bad.status(), (0, 0x02));
}

#[test]
fn mandatory_features_round_trip_and_unsupported_ones_are_invalid_field() {
    let mut rig = Rig::enabled();
    let set = |rig: &mut Rig, fid: u32, value: u32| {
        rig.admin(Sqe::new(OPC_SET_FEATURES).nsid(0).cdw(fid, value))
    };
    let get = |rig: &mut Rig, fid: u32, value: u32| {
        rig.admin(Sqe::new(OPC_GET_FEATURES).nsid(0).cdw(fid, value))
    };
    // Arbitration (01h): reserved bits are dropped.
    assert_eq!(get(&mut rig, 0x01, 0).dw0, 7);
    assert_eq!(set(&mut rig, 0x01, 0x0302_0101).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x01, 0).dw0, 0x0302_0101);
    // Power Management (02h): one power state.
    assert_eq!(set(&mut rig, 0x02, 0).status(), (0, 0));
    assert_eq!(set(&mut rig, 0x02, 1).status(), (0, 0x02));
    assert_eq!(get(&mut rig, 0x02, 0).dw0, 0);
    // Temperature Threshold (04h): composite sensor, over and under.
    assert_eq!(get(&mut rig, 0x04, 0).dw0, 343);
    assert_eq!(set(&mut rig, 0x04, 350).status(), (0, 0));
    assert_eq!(set(&mut rig, 0x04, (1 << 20) | 280).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x04, 0).dw0, 350);
    assert_eq!(get(&mut rig, 0x04, 1 << 20).dw0, 280);
    assert_eq!(
        set(&mut rig, 0x04, (1 << 16) | 300).status(),
        (0, 0x02),
        "sensor 1 absent"
    );
    // Error Recovery (05h): TLER, no DULBE.
    assert_eq!(set(&mut rig, 0x05, 50).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x05, 0).dw0, 50);
    assert_eq!(set(&mut rig, 0x05, 1 << 16).status(), (0, 0x02));
    let bad_ns = rig.admin(Sqe::new(OPC_SET_FEATURES).nsid(9).cdw(0x05, 0));
    assert_eq!(bad_ns.status(), (0, 0x0B));
    // Interrupt Coalescing (08h) and Interrupt Vector Configuration (09h).
    assert_eq!(set(&mut rig, 0x08, 0x0A05).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x08, 0).dw0, 0x0A05);
    assert_eq!(set(&mut rig, 0x09, 1 << 16).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x09, 0).dw0, 1 << 16);
    assert_eq!(
        set(&mut rig, 0x09, 1).status(),
        (0, 0x02),
        "only vector 0 exists"
    );
    // Write Atomicity Normal (0Ah) and Asynchronous Event Configuration (0Bh).
    assert_eq!(set(&mut rig, 0x0A, 1).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x0A, 0).dw0, 1);
    assert_eq!(set(&mut rig, 0x0B, 0x1F).status(), (0, 0));
    assert_eq!(get(&mut rig, 0x0B, 0).dw0, 0x1F);
    assert_eq!(set(&mut rig, 0x0B, 0x100).status(), (0, 0x02), "OAES = 0");
    // Not implemented: Volatile Write Cache (06h, VWC = 0), LBA Range Type (03h), APST (0Ch).
    for fid in [0x03, 0x06, 0x0C, 0x80, 0xC0] {
        assert_eq!(
            set(&mut rig, fid, 0).status(),
            (0, 0x02),
            "set fid {fid:#x}"
        );
        assert_eq!(
            get(&mut rig, fid, 0).status(),
            (0, 0x02),
            "get fid {fid:#x}"
        );
    }
}

#[test]
fn get_features_select_and_set_features_save_are_limited_to_current_and_default() {
    let mut rig = Rig::enabled();
    rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x08, 0x0A05));
    let current = rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x08, 0));
    let default = rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x08 | (1 << 8), 0));
    assert_eq!((current.dw0, default.dw0), (0x0A05, 0));
    for select in [2u32, 3] {
        let cqe = rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x08 | (select << 8), 0));
        assert_eq!(cqe.status(), (0, 0x02), "select {select}");
    }
    let save = rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x08 | (1 << 31), 1));
    assert_eq!(save.status(), (0, 0x02), "nothing is persistent");
}

#[test]
fn features_return_to_their_defaults_after_a_controller_reset() {
    let mut rig = Rig::enabled();
    rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x08, 0x0A05));
    rig.admin(Sqe::new(OPC_SET_FEATURES).cdw(0x07, 0));
    rig.set_dword(0x14, 0);
    rig.sq_tail = 0;
    rig.cq_head = 0;
    rig.cq_phase = true;
    rig.mem_write(ACQ, &[0u8; CQE_BYTES * 2]);
    rig.enable();
    assert_eq!(rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x08, 0)).dw0, 0);
    assert_eq!(
        rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0)).dw0,
        3 | (3 << 16)
    );
}

// ---------------------------------------------------------------- log pages

#[test]
fn error_information_log_records_failed_commands_newest_first() {
    let mut rig = Rig::enabled();
    let (_, page) = rig.log_page(0x01, 4096, 0, true);
    assert!(
        page.iter().all(|&b| b == 0),
        "no errors yet: all entries unused"
    );
    let (cid_a, a) = rig.admin_try(Sqe::new(0x7F)); // invalid opcode
    let (cid_b, b) = rig.admin_try(Sqe::new(OPC_IDENTIFY).nsid(7).prp(DATA, 0).cdw(0, 0));
    assert_eq!(a.unwrap().status(), (0, 0x01));
    assert_eq!(b.unwrap().status(), (0, 0x0B));
    let (cqe, page) = rig.log_page(0x01, 4096, 0, true);
    assert_eq!(cqe.status(), (0, 0));
    // Entry 0 is the newest error (Identify with NSID 7), entry 1 the opcode.
    assert_eq!(u64_at(&page, 0), 2, "Error Count");
    assert_eq!(u16_at(&page, 8), 0, "SQID");
    assert_eq!(u16_at(&page, 10), cid_b, "CID");
    let status = u16_at(&page, 12);
    assert_eq!((status >> 1) & 0xFF, 0x0B, "SC");
    assert_eq!((status >> 9) & 0x7, 0, "SCT");
    assert_eq!(status >> 15, 1, "DNR");
    assert_eq!(u16_at(&page, 14), 0xFFFF, "parameter location not tracked");
    assert_eq!(u32_at(&page, 24), 7, "NSID of the failed command");
    assert_eq!(u64_at(&page, 64), 1);
    assert_eq!(u16_at(&page, 64 + 10), cid_a);
    assert_eq!(u32_at(&page, 64 + 24), 0xFFFF_FFFF);
    assert!(page[128..].iter().all(|&b| b == 0));
    // A short read returns the prefix.
    let (_, short) = rig.log_page(0x01, 64, 0, true);
    assert_eq!(short, page[..64].to_vec());
}

#[test]
fn smart_health_log_is_a_512_byte_structure_with_live_values() {
    let mut rig = Rig::enabled();
    let (cqe, page) = rig.log_page(0x02, 512, 0xFFFF_FFFF, true);
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(page[0], 0, "no critical warning");
    assert_eq!(u16_at(&page, 1), 300, "composite temperature in kelvin");
    assert_eq!(page[3], 100, "available spare");
    assert_eq!(page[4], 10, "available spare threshold");
    assert_eq!(page[5], 0, "percentage used");
    assert_eq!(u64_at(&page, 112), 1, "power cycles");
    assert_eq!(u64_at(&page, 176), 0, "error log entries");
    for field in [32, 48, 64, 80, 96, 128, 144, 160] {
        assert_eq!(u64_at(&page, field), 0, "no I/O performed: field {field}");
    }
    // The error count follows the Error Information log.
    rig.admin(Sqe::new(0x7F));
    let (_, page) = rig.log_page(0x02, 512, 0, true);
    assert_eq!(u64_at(&page, 176), 1);
    assert!(page[216..].iter().all(|&b| b == 0));
    // Offsets and namespace rules.
    rig.mem_write(DATA, &[0xAA; 8]);
    let at_offset = Sqe::new(OPC_GET_LOG_PAGE)
        .prp(DATA, 0)
        .cdw(0x02 | (1 << 16), 0)
        .with_offset(112);
    assert_eq!(rig.admin(at_offset).status(), (0, 0));
    let power_cycles = rig.mem_read(DATA, 8);
    assert_eq!(
        u64::from_le_bytes(power_cycles.try_into().unwrap()),
        1,
        "LPO selects power cycles"
    );
    let (cqe, _) = rig.log_page(0x02, 512, 1, true);
    assert_eq!(
        cqe.status(),
        (0, 0x02),
        "LPA bit 0 clear: controller-wide only"
    );
}

#[test]
fn firmware_slot_log_names_the_active_revision() {
    let mut rig = Rig::enabled();
    let (cqe, page) = rig.log_page(0x03, 512, 0, true);
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(page[0], 0x01, "AFI: slot 1 active");
    assert_eq!(&page[8..16], b"0.1.0   ");
    assert!(page[1..8].iter().all(|&b| b == 0));
    assert!(page[16..].iter().all(|&b| b == 0), "slots 2-7 empty");
}

#[test]
fn log_page_requests_outside_the_defined_pages_are_refused() {
    let mut rig = Rig::enabled();
    let (cqe, _) = rig.log_page(0x7F, 512, 0, true);
    assert_eq!(cqe.status(), (1, 0x09), "Invalid Log Page");
    // Longer than the page, offset past its end, offset not dword aligned.
    let (cqe, _) = rig.log_page(0x02, 1024, 0, true);
    assert_eq!(cqe.status(), (0, 0x02));
    let long = Sqe::new(OPC_GET_LOG_PAGE)
        .prp(DATA, 0)
        .cdw(0x02 | (1 << 16), 0);
    assert_eq!(rig.admin(long.with_offset(512)).status(), (0, 0x02));
    assert_eq!(rig.admin(long.with_offset(2)).status(), (0, 0x02));
}

// ---------------------------------------------------------------- AER, abort

#[test]
fn async_event_request_is_accepted_and_stays_outstanding() {
    let mut rig = Rig::enabled();
    let (_, none) = rig.admin_try(Sqe::new(OPC_AER));
    assert!(none.is_none(), "no completion without an event");
    // The command was consumed: SQHD advanced and later commands still complete.
    let cqe = rig.admin(Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0));
    assert_eq!(cqe.sqhd, 2);
    // AERL = 3: four may be outstanding, the fifth is refused.
    for _ in 0..3 {
        assert!(rig.admin_try(Sqe::new(OPC_AER)).1.is_none());
    }
    let limit = rig.admin(Sqe::new(OPC_AER));
    assert_eq!(
        limit.status(),
        (1, 0x05),
        "Asynchronous Event Request Limit Exceeded"
    );
}

#[test]
fn async_event_completes_with_event_type_info_and_log_page() {
    let mut rig = Rig::enabled();
    let (aer, _) = rig.admin_try(Sqe::new(OPC_AER));
    rig.set_dword(ADMIN_SQ_DOORBELL, 0xFFFF);
    rig.service();
    let event = rig.poll_admin().expect("event completion");
    assert_eq!(event.cid, aer);
    // Type 0 (error status), info 1 (invalid doorbell write value), log page 1.
    assert_eq!(event.dw0, 0x0001_0100);
    assert_eq!(event.status(), (0, 0));
    // A second event of the same type stays masked until the log page is read
    // with RAE = 0; a queued AER then waits again.
    let (second, _) = rig.admin_try(Sqe::new(OPC_AER));
    rig.set_dword(ADMIN_SQ_DOORBELL, 0xFFFF);
    rig.service();
    assert!(rig.poll_admin().is_none(), "masked");
    rig.log_page(0x01, 64, 0, false);
    rig.set_dword(ADMIN_SQ_DOORBELL, 0xFFFF);
    rig.service();
    let again = rig.poll_admin().expect("unmasked event");
    assert_eq!((again.cid, again.dw0), (second, 0x0001_0100));
}

#[test]
fn an_event_that_happens_first_completes_the_next_aer_at_once() {
    let mut rig = Rig::enabled();
    rig.set_dword(ADMIN_CQ_DOORBELL, 0xFFFF);
    let (cid, cqe) = rig.admin_try(Sqe::new(OPC_AER));
    let cqe = cqe.expect("queued event completes the request");
    assert_eq!((cqe.cid, cqe.dw0), (cid, 0x0001_0100));
}

#[test]
fn abort_reports_the_target_as_not_aborted() {
    let mut rig = Rig::enabled();
    // Abort targets (SQID 0, CID 0x1234); it has completed or never existed.
    let cqe = rig.admin(Sqe::new(OPC_ABORT).cdw(0x1234 << 16, 0));
    assert_eq!(cqe.status(), (0, 0));
    assert_eq!(cqe.dw0 & 1, 1, "command not aborted");
    // An unknown submission queue is an invalid field.
    let cqe = rig.admin(Sqe::new(OPC_ABORT).cdw(3, 0));
    assert_eq!(cqe.status(), (0, 0x02));
    let cqe = rig.admin(Sqe::new(OPC_ABORT).cdw(0x40, 0));
    assert_eq!(cqe.status(), (0, 0x02));
}

#[test]
fn commands_the_controller_does_not_implement_are_invalid_opcodes() {
    let mut rig = Rig::enabled();
    for opcode in [
        0x03, 0x0D, 0x10, 0x11, 0x14, 0x15, 0x18, 0x7C, 0x80, 0x81, 0x84, 0xC0, 0xFF,
    ] {
        let cqe = rig.admin(Sqe::new(opcode));
        assert_eq!(cqe.status(), (0, 0x01), "admin opcode {opcode:#x}");
        assert!(cqe.dnr);
    }
    let mut raw = Sqe::new(OPC_IDENTIFY).prp(DATA, 0).cdw(1, 0).raw(0x55);
    raw[1] |= 0x01; // FUSE = first command
    rig.mem_write(ASQ + u64::from(rig.sq_tail) * SQE_BYTES as u64, &raw);
    rig.sq_tail += 1;
    rig.set_dword(ADMIN_SQ_DOORBELL, u32::from(rig.sq_tail));
    rig.service();
    assert_eq!(
        rig.poll_admin().unwrap().status(),
        (0, 0x02),
        "fused operations"
    );
}

#[test]
fn service_does_nothing_while_the_controller_is_disabled() {
    let mut rig = rig();
    assert_eq!(rig.service(), NvmeServiceReport::default());
    assert!(!rig.nvme.doorbell_pending());
}

#[test]
fn a_completion_queue_outside_ram_sets_the_fatal_status() {
    let mut rig = rig();
    rig.set_dword(0x24, 0x000F_000F);
    rig.write(0x28, AccessWidth::Qword, ASQ).unwrap();
    rig.write(0x30, AccessWidth::Qword, RAM_BYTES + 0x10_0000)
        .unwrap();
    rig.set_dword(0x14, CC_ENABLE);
    rig.mem_write(ASQ, &Sqe::new(OPC_GET_FEATURES).cdw(0x07, 0).raw(1));
    rig.set_dword(ADMIN_SQ_DOORBELL, 1);
    let report = rig.service();
    assert_eq!(report.completions, 0);
    assert_eq!(rig.dword(0x1C) & 0b10, 0b10, "CSTS.CFS");
}

#[test]
fn config_is_validated() {
    let bad = |config: NvmeConfig| NvmeDevice::new_with_handle(config).err();
    assert_eq!(
        bad(NvmeConfig {
            namespace_sectors: 0,
            ..config()
        }),
        Some(NvmeConfigError::EmptyNamespace)
    );
    let mut text = config();
    text.serial[0] = 0x07;
    assert_eq!(bad(text), Some(NvmeConfigError::NotAscii("serial")));
    let mut uuid = config();
    uuid.namespace_uuid = [0; 16];
    assert_eq!(bad(uuid), Some(NvmeConfigError::ZeroUuid("namespace_uuid")));
    assert!(bad(config()).is_none());
}
