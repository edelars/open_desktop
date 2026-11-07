//! Register file, controller enable/reset state machine, doorbells and the
//! command servicing loop of the minimal NVMe controller (S7-16 slice A).
//!
//! The device is a `BusDevice` on a fixed MMIO window. A `BusDevice` cannot
//! reach guest RAM, so doorbell writes only record the new queue pointers and
//! mark work pending; the machine owner calls [`NvmeHandle::service`] between
//! guest quanta, which fetches commands, executes them and posts completions
//! (`docs/specs/NVME_DEVICE.md`, "Servicing model").
//!
//! Register layout and bit positions: cross-checked with Linux
//! `include/linux/nvme.h` (master, 2026-11-07), not with the text of NVMe Base
//! 2.4. Verified: CAP/VS/INTMS/INTMC/CC/CSTS/NSSR/AQA/ASQ/ACQ and doorbells
//! (0x1000); CC EN 0, CSS 4, MPS 7, AMS 11, SHN 14, IOSQES 16, IOCQES 20; CSTS
//! RDY, CFS, SHST; CAP MQES[15:0], TO[31:24], CSS, MPSMIN 48. Everything else
//! (CAP.CQR bit 16, AER info 0/1, PRP rules, feature defaults, error behaviour,
//! which commands are mandatory, `VS` 2.0 versus 2.4) is `NOT VERIFIED`
//! (`docs/specs/NVME_DEVICE.md` sections 1 and 11).

use super::admin::{self, AerState, ErrorLog, Features};
use super::io;
use super::queues::{
    BusDma, Command, Completion, CompletionQueue, GuestDma, MAX_QUEUE_ENTRIES, Outcome, SQE_BYTES,
    SubmissionQueue, encode_cqe,
};
use crate::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalRange, IoSpace, MachineBus,
};
use std::fmt;
use std::sync::{Arc, Mutex};

/// Number of I/O queue pairs the controller offers (queue ids `1..=MAX_IO_QUEUES`).
pub(crate) const MAX_IO_QUEUES: usize = 4;
/// Queue slots including the admin queue pair (queue id 0).
pub(crate) const QUEUE_SLOTS: usize = MAX_IO_QUEUES + 1;
/// Version register: major 2, minor 0, tertiary 0.
pub(crate) const VERSION: u32 = 0x0002_0000;

/// Offsets of the controller properties (NVMe Base, "Controller Registers").
mod reg {
    pub(super) const CAP: u64 = 0x00;
    pub(super) const VS: u64 = 0x08;
    pub(super) const INTMS: u64 = 0x0C;
    pub(super) const INTMC: u64 = 0x10;
    pub(super) const CC: u64 = 0x14;
    pub(super) const CSTS: u64 = 0x1C;
    pub(super) const NSSR: u64 = 0x20;
    pub(super) const AQA: u64 = 0x24;
    pub(super) const ASQ: u64 = 0x28;
    pub(super) const ACQ: u64 = 0x30;
    pub(super) const DOORBELLS: u64 = 0x1000;
}

/// `CAP`: MQES = 1023 (0's based), CQR = 1, AMS = 0 (round robin only),
/// TO = 1 (500 ms units), DSTRD = 0, NSSRS = 0, CSS bit 37 (NVM command set),
/// MPSMIN = MPSMAX = 0 (4 KiB), no BPS/CPS/PMRS/CMBS/NSSS/CRMS.
const CAP_VALUE: u64 = (MAX_QUEUE_ENTRIES as u64 - 1) | (1 << 16) | (1 << 24) | (1 << 37);

const CC_EN: u32 = 1;
/// Writable `CC` bits: EN, CSS, MPS, AMS, SHN, IOSQES, IOCQES (CRIME is not implemented).
const CC_WRITABLE: u32 = 0x00FF_FFF1;
const CSTS_RDY: u32 = 1;
const CSTS_CFS: u32 = 1 << 1;
const CSTS_SHST_COMPLETE: u32 = 0b10 << 2;

/// Identity and size of the single namespace and of the controller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NvmeConfig {
    /// Namespace size in 512-byte sectors (`NSZE` = `NCAP` = `NUSE`); not zero.
    pub namespace_sectors: u64,
    /// Serial number (`SN`), ASCII `0x20..=0x7E`, space padded.
    pub serial: [u8; 20],
    /// Model number (`MN`), ASCII `0x20..=0x7E`, space padded.
    pub model: [u8; 40],
    /// Firmware revision (`FR`), ASCII `0x20..=0x7E`, space padded.
    pub firmware_revision: [u8; 8],
    /// Identifies the NVM subsystem (`SUBNQN`); not all zero.
    pub subsystem_uuid: [u8; 16],
    /// Namespace UUID returned in the namespace identification descriptor
    /// list and as `NGUID`; not all zero.
    pub namespace_uuid: [u8; 16],
}

fn padded<const N: usize>(text: &[u8]) -> [u8; N] {
    let mut out = [b' '; N];
    out[..text.len().min(N)].copy_from_slice(&text[..text.len().min(N)]);
    out
}

impl Default for NvmeConfig {
    fn default() -> Self {
        Self {
            // 1 GiB.
            namespace_sectors: 2 * 1024 * 1024,
            serial: padded(b"OD-NVME-0001"),
            model: padded(b"Open Desktop Virtual NVMe"),
            firmware_revision: padded(b"0.1.0"),
            subsystem_uuid: *b"OpenDesktopNVMeS",
            namespace_uuid: *b"OpenDesktopNVMeN",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NvmeConfigError {
    /// `namespace_sectors` is zero.
    EmptyNamespace,
    /// The named text field has a byte outside `0x20..=0x7E`.
    NotAscii(&'static str),
    /// The named UUID is all zero.
    ZeroUuid(&'static str),
}

impl fmt::Display for NvmeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid NVMe configuration: {self:?}")
    }
}
impl std::error::Error for NvmeConfigError {}

impl NvmeConfig {
    fn validate(&self) -> Result<(), NvmeConfigError> {
        if self.namespace_sectors == 0 {
            return Err(NvmeConfigError::EmptyNamespace);
        }
        let ascii = |bytes: &[u8]| bytes.iter().all(|b| (0x20..=0x7E).contains(b));
        for (name, bytes) in [
            ("serial", &self.serial[..]),
            ("model", &self.model[..]),
            ("firmware_revision", &self.firmware_revision[..]),
        ] {
            if !ascii(bytes) {
                return Err(NvmeConfigError::NotAscii(name));
            }
        }
        for (name, uuid) in [
            ("subsystem_uuid", &self.subsystem_uuid),
            ("namespace_uuid", &self.namespace_uuid),
        ] {
            if uuid.iter().all(|&b| b == 0) {
                return Err(NvmeConfigError::ZeroUuid(name));
            }
        }
        Ok(())
    }
}

/// Work done by one [`NvmeHandle::service`] call.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NvmeServiceReport {
    /// Commands fetched from submission queues and executed.
    pub commands: u32,
    /// Completion queue entries written (including delayed AER completions).
    pub completions: u32,
}

#[derive(Debug)]
pub(crate) struct NvmeState {
    pub(super) config: NvmeConfig,
    cc: u32,
    csts: u32,
    intms: u32,
    aqa: u32,
    asq: u64,
    acq: u64,
    pub(super) sqs: [Option<SubmissionQueue>; QUEUE_SLOTS],
    pub(super) cqs: [Option<CompletionQueue>; QUEUE_SLOTS],
    pub(super) features: Features,
    pub(super) errors: ErrorLog,
    pub(super) aer: AerState,
    pending: bool,
}

impl NvmeState {
    fn new(config: NvmeConfig) -> Self {
        Self {
            config,
            cc: 0,
            csts: 0,
            intms: 0,
            aqa: 0,
            asq: 0,
            acq: 0,
            sqs: [None; QUEUE_SLOTS],
            cqs: [None; QUEUE_SLOTS],
            features: Features::default(),
            errors: ErrorLog::default(),
            aer: AerState::default(),
            pending: false,
        }
    }

    pub(super) const fn ready(&self) -> bool {
        self.csts & CSTS_RDY != 0 && self.csts & CSTS_CFS == 0
    }

    pub(super) fn set_fatal(&mut self) {
        self.csts |= CSTS_CFS;
    }

    /// Controller reset (`CC.EN` 1 to 0): queues, features and outstanding
    /// events are lost; the error log and counters are kept.
    fn reset(&mut self) {
        self.sqs = [None; QUEUE_SLOTS];
        self.cqs = [None; QUEUE_SLOTS];
        self.features = Features::default();
        self.aer = AerState::default();
        self.csts = 0;
        self.pending = false;
    }

    /// `CC.EN` 0 to 1: validate the configuration and create the admin queues.
    fn enable(&mut self) {
        let cc = self.cc;
        let css = (cc >> 4) & 0x7;
        let mps = (cc >> 7) & 0xF;
        let ams = (cc >> 11) & 0x7;
        let iosqes = (cc >> 16) & 0xF;
        let iocqes = (cc >> 20) & 0xF;
        let sq_entries = (self.aqa & 0xFFF) + 1;
        let cq_entries = ((self.aqa >> 16) & 0xFFF) + 1;
        // NOT VERIFIED against NVMe Base 2.4: проверка IOSQES/IOCQES при EN=1 против при Create I/O SQ/CQ
        // (likely deviation: the specification, unconfirmed, checks these fields when an
        // I/O queue is created, not on enable; NVME_DEVICE.md section 11).
        let valid = css == 0
            && mps == 0
            && ams == 0
            && iosqes == 6
            && iocqes == 4
            && (2..=MAX_QUEUE_ENTRIES).contains(&sq_entries)
            && (2..=MAX_QUEUE_ENTRIES).contains(&cq_entries);
        if !valid {
            self.csts = CSTS_CFS;
            return;
        }
        self.sqs[0] = Some(SubmissionQueue::new(self.asq, sq_entries as u16, 0));
        self.cqs[0] = Some(CompletionQueue::new(self.acq, cq_entries as u16, false));
        self.csts = CSTS_RDY;
    }

    /// Controller property span containing `offset`: `(start, length)`; the
    /// reserved block `0x38..0x1000` is one span.
    fn span(offset: u64) -> (u64, u64) {
        match offset {
            0x00..=0x07 => (reg::CAP, 8),
            0x08..=0x0B => (reg::VS, 4),
            0x0C..=0x0F => (reg::INTMS, 4),
            0x10..=0x13 => (reg::INTMC, 4),
            0x14..=0x17 => (reg::CC, 4),
            0x18..=0x1B => (0x18, 4),
            0x1C..=0x1F => (reg::CSTS, 4),
            0x20..=0x23 => (reg::NSSR, 4),
            0x24..=0x27 => (reg::AQA, 4),
            0x28..=0x2F => (reg::ASQ, 8),
            0x30..=0x37 => (reg::ACQ, 8),
            // Reserved (CMB, boot partition, PMR and the like are not implemented).
            _ => (0x38, reg::DOORBELLS - 0x38),
        }
    }

    fn check_register_access(offset: u64, width: AccessWidth) -> Result<(u64, u64), DeviceError> {
        if matches!(width, AccessWidth::Byte | AccessWidth::Word) {
            return Err(DeviceError::UnsupportedWidth);
        }
        let bytes = u64::from(width.bytes());
        if !offset.is_multiple_of(bytes) {
            return Err(DeviceError::Misaligned);
        }
        let (start, len) = Self::span(offset);
        if offset + bytes > start + len {
            return Err(DeviceError::CrossesRegister);
        }
        Ok((start, len))
    }

    fn read_register(&self, offset: u64, width: AccessWidth) -> Result<u64, DeviceError> {
        let (start, len) = Self::check_register_access(offset, width)?;
        let value = match start {
            reg::CAP => CAP_VALUE,
            reg::VS => u64::from(VERSION),
            reg::INTMS | reg::INTMC => u64::from(self.intms),
            reg::CC => u64::from(self.cc),
            reg::CSTS => u64::from(self.csts),
            reg::AQA => u64::from(self.aqa),
            reg::ASQ => self.asq,
            reg::ACQ => self.acq,
            // NSSR is not implemented (CAP.NSSRS = 0) and reads as zero.
            _ => 0,
        };
        // Dword halves of the 64-bit properties; every other span starts at its register.
        let shift = if len == 8 { (offset - start) * 8 } else { 0 };
        Ok(value >> shift)
    }

    fn write_register(
        &mut self,
        offset: u64,
        width: AccessWidth,
        value: u64,
    ) -> Result<(), DeviceError> {
        let (start, len) = Self::check_register_access(offset, width)?;
        let enabled = self.cc & CC_EN != 0;
        let shift = if len == 8 { (offset - start) * 8 } else { 0 };
        let field_mask = match width {
            AccessWidth::Qword => u64::MAX,
            _ => 0xFFFF_FFFFu64 << shift,
        };
        let merge = |old: u64| (old & !field_mask) | ((value << shift) & field_mask);
        match start {
            reg::CAP | reg::VS | reg::CSTS => Err(DeviceError::ReadOnly),
            reg::INTMS => {
                self.intms |= value as u32;
                Ok(())
            }
            reg::INTMC => {
                self.intms &= !(value as u32);
                Ok(())
            }
            reg::CC => {
                self.write_cc(value as u32);
                Ok(())
            }
            reg::AQA | reg::ASQ | reg::ACQ if enabled => Err(DeviceError::Busy),
            reg::AQA => {
                // ASQS and ACQS only; the other bits are reserved.
                self.aqa = (value as u32) & 0x0FFF_0FFF;
                Ok(())
            }
            reg::ASQ => {
                self.asq = merge(self.asq) & !0xFFF;
                Ok(())
            }
            reg::ACQ => {
                self.acq = merge(self.acq) & !0xFFF;
                Ok(())
            }
            // NSSR and the reserved block ignore writes.
            _ => Ok(()),
        }
    }

    fn write_cc(&mut self, value: u32) {
        let was_enabled = self.cc & CC_EN != 0;
        let value = value & CC_WRITABLE;
        if !was_enabled {
            self.cc = value;
            if value & CC_EN != 0 {
                self.enable();
            }
            return;
        }
        if value & CC_EN == 0 {
            self.cc = value;
            self.reset();
            return;
        }
        // Enabled and staying enabled: only the shutdown notification changes.
        self.cc = (self.cc & !(0b11 << 14)) | (value & (0b11 << 14));
        if value & (0b11 << 14) != 0 {
            self.csts = (self.csts & !(0b11 << 2)) | CSTS_SHST_COMPLETE;
        }
    }

    fn read_doorbell(offset: u64) -> Result<u64, DeviceError> {
        let qid = (offset - reg::DOORBELLS) / 8;
        if qid as usize >= QUEUE_SLOTS {
            return Err(DeviceError::UnknownOffset);
        }
        Err(DeviceError::WriteOnly)
    }

    fn write_doorbell(
        &mut self,
        offset: u64,
        width: AccessWidth,
        value: u64,
    ) -> Result<(), DeviceError> {
        if width != AccessWidth::Dword {
            return Err(DeviceError::UnsupportedWidth);
        }
        if !offset.is_multiple_of(4) {
            return Err(DeviceError::Misaligned);
        }
        let index = (offset - reg::DOORBELLS) / 4;
        let qid = (index / 2) as usize;
        let is_cq = index % 2 == 1;
        if qid >= QUEUE_SLOTS {
            return Err(DeviceError::UnknownOffset);
        }
        if !self.ready() {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let value = (value & 0xFFFF) as u16;
        if is_cq {
            match self.cqs[qid].as_mut() {
                None => admin::report_invalid_doorbell_register(self, qid as u16),
                Some(cq) if value >= cq.entries => {
                    admin::report_invalid_doorbell_value(self, qid as u16);
                }
                Some(cq) => {
                    cq.head = value;
                    self.pending = true;
                }
            }
        } else {
            match self.sqs[qid].as_mut() {
                None => admin::report_invalid_doorbell_register(self, qid as u16),
                Some(sq) if value >= sq.entries => {
                    admin::report_invalid_doorbell_value(self, qid as u16);
                }
                Some(sq) => {
                    sq.tail = value;
                    self.pending = true;
                }
            }
        }
        Ok(())
    }

    /// Writes one completion entry of command `cid` (of submission queue `sqid`)
    /// to completion queue `cqid`. Returns `false` when the queue is full or the
    /// host memory is not RAM (the latter also sets `CSTS.CFS`).
    fn post(
        &mut self,
        dma: &mut dyn GuestDma,
        cqid: usize,
        sqid: u16,
        cid: u16,
        completion: Completion,
    ) -> bool {
        let sq_head = self.sqs[usize::from(sqid)].map_or(0, |sq| sq.head);
        let Some(cq) = self.cqs[cqid].as_mut() else {
            return false;
        };
        if cq.is_full() {
            return false;
        }
        let raw = encode_cqe(completion, sq_head, sqid, cid, cq.phase);
        if dma.write(cq.tail_address(), &raw).is_err() {
            self.set_fatal();
            return false;
        }
        cq.advance_tail();
        true
    }

    /// Fetches and executes at most one command of submission queue `slot`.
    fn run_one(
        &mut self,
        slot: usize,
        dma: &mut dyn GuestDma,
        report: &mut NvmeServiceReport,
    ) -> bool {
        let Some(sq) = self.sqs[slot] else {
            return false;
        };
        if !sq.has_work() {
            return false;
        }
        let cqid = usize::from(sq.cqid);
        match self.cqs[cqid] {
            Some(cq) if !cq.is_full() => {}
            _ => return false,
        }
        let mut raw = [0u8; SQE_BYTES];
        if dma.read(sq.head_address(), &mut raw).is_err() {
            self.set_fatal();
            return false;
        }
        if let Some(queue) = self.sqs[slot].as_mut() {
            queue.advance_head();
        }
        let command = Command::decode(&raw);
        report.commands += 1;
        let outcome = if slot == 0 {
            admin::execute(self, &command, dma)
        } else {
            io::execute(&command)
        };
        if let Outcome::Done(completion) = outcome {
            if !completion.status.is_success() {
                let phase = self.cqs[cqid].is_some_and(|cq| cq.phase);
                admin::log_command_error(self, slot as u16, &command, completion.status, phase);
            }
            if self.post(dma, cqid, slot as u16, command.cid, completion) {
                report.completions += 1;
            }
        }
        true
    }

    /// Posts completions of `Asynchronous Event Request` commands whose event arrived.
    fn flush_events(&mut self, dma: &mut dyn GuestDma, report: &mut NvmeServiceReport) -> bool {
        let mut progressed = false;
        while let Some((cid, dw0)) = self.aer.next_ready() {
            if self.post(dma, 0, 0, cid, Completion::ok(dw0)) {
                self.aer.pop_ready();
                report.completions += 1;
                progressed = true;
            } else {
                break;
            }
        }
        progressed
    }

    fn service(&mut self, dma: &mut dyn GuestDma) -> NvmeServiceReport {
        let mut report = NvmeServiceReport::default();
        self.pending = false;
        if !self.ready() {
            return report;
        }
        loop {
            let mut progressed = self.flush_events(dma, &mut report);
            for slot in 0..QUEUE_SLOTS {
                while self.ready() && self.run_one(slot, dma, &mut report) {
                    progressed = true;
                }
            }
            if !progressed || !self.ready() {
                break;
            }
        }
        report
    }
}

fn lock(state: &Mutex<NvmeState>) -> std::sync::MutexGuard<'_, NvmeState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The NVMe controller as an MMIO `BusDevice`.
pub struct NvmeDevice {
    state: Arc<Mutex<NvmeState>>,
}

/// Host-side handle aliasing the controller state; drives command servicing.
#[derive(Clone, Debug)]
pub struct NvmeHandle {
    state: Arc<Mutex<NvmeState>>,
}

impl NvmeDevice {
    /// Creates a powered-off controller (`CSTS.RDY = 0`) and its handle.
    pub fn new_with_handle(config: NvmeConfig) -> Result<(Self, NvmeHandle), NvmeConfigError> {
        config.validate()?;
        let state = Arc::new(Mutex::new(NvmeState::new(config)));
        Ok((
            Self {
                state: Arc::clone(&state),
            },
            NvmeHandle { state },
        ))
    }
}

impl NvmeHandle {
    /// A doorbell write since the last [`service`](Self::service) call: the
    /// machine owner should end the guest quantum and service the controller.
    pub fn doorbell_pending(&self) -> bool {
        lock(&self.state).pending
    }

    /// `CSTS.RDY = 1` and no fatal status.
    pub fn is_ready(&self) -> bool {
        lock(&self.state).ready()
    }

    /// Executes every submitted command that fits the completion queues and
    /// posts the completions. Guest memory is touched only through `bus` and
    /// only in RAM; `committed_write` receives every written range.
    pub fn service(
        &self,
        bus: &mut MachineBus,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> NvmeServiceReport {
        let mut dma = BusDma {
            bus,
            committed: committed_write,
        };
        lock(&self.state).service(&mut dma)
    }
}

impl BusDevice for NvmeDevice {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        if access.offset >= reg::DOORBELLS {
            return NvmeState::read_doorbell(access.offset);
        }
        lock(&self.state).read_register(access.offset, access.width)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let mut state = lock(&self.state);
        if access.offset >= reg::DOORBELLS {
            state.write_doorbell(access.offset, access.width, value)
        } else {
            state.write_register(access.offset, access.width, value)
        }
    }
}
