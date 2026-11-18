//! Admin command set of the minimal NVMe controller (S7-16 slice A): queue
//! management, `Identify`, `Get`/`Set Features`, `Get Log Page`,
//! `Asynchronous Event Request` and `Abort`.
//!
//! The specification text could not be extracted (see
//! `docs/specs/NVME_DEVICE.md`, section 1). Cross-checked with Linux
//! `include/linux/nvme.h` (master, 2026-11-07), not with the text of NVMe Base
//! 2.4: admin opcodes 0x00/01/02/04/05/06/08/09/0A/0C; CNS 0-3; FID
//! 0x01/02/04/05/07/08/09/0A/0B; LID 0x01-0x03; generic statuses
//! 0x01/02/04/0B/0C/13 and command specific 0x100/101/102/105/108/109/10C;
//! Identify Controller and Namespace offsets; Error log, SMART and Firmware
//! Slot offsets. `NOT VERIFIED`: AER info 0/1, PRP rules, feature defaults,
//! error behaviour, which commands are mandatory, LID 0x00 and 0x05.

use super::controller::{MAX_IO_QUEUES, NvmeServiceReport, NvmeState, QUEUE_SLOTS, VERSION};
use super::io;
use super::queues::{
    CQE_BYTES, Command, Completion, CompletionQueue, GuestDma, MAX_QUEUE_ENTRIES, Outcome,
    PAGE_BYTES, SQE_BYTES, Status, SubmissionQueue, write_to_host,
};
use std::collections::VecDeque;

const OPC_DELETE_SQ: u8 = 0x00;
const OPC_CREATE_SQ: u8 = 0x01;
const OPC_GET_LOG_PAGE: u8 = 0x02;
const OPC_DELETE_CQ: u8 = 0x04;
const OPC_CREATE_CQ: u8 = 0x05;
const OPC_IDENTIFY: u8 = 0x06;
const OPC_ABORT: u8 = 0x08;
const OPC_SET_FEATURES: u8 = 0x09;
const OPC_GET_FEATURES: u8 = 0x0A;
const OPC_ASYNC_EVENT_REQUEST: u8 = 0x0C;

const CNS_NAMESPACE: u32 = 0x00;
const CNS_CONTROLLER: u32 = 0x01;
const CNS_ACTIVE_NAMESPACES: u32 = 0x02;
const CNS_NAMESPACE_DESCRIPTORS: u32 = 0x03;

const FID_ARBITRATION: u8 = 0x01;
const FID_POWER_MANAGEMENT: u8 = 0x02;
const FID_TEMPERATURE_THRESHOLD: u8 = 0x04;
const FID_ERROR_RECOVERY: u8 = 0x05;
const FID_NUMBER_OF_QUEUES: u8 = 0x07;
const FID_INTERRUPT_COALESCING: u8 = 0x08;
const FID_INTERRUPT_VECTOR_CONFIG: u8 = 0x09;
const FID_WRITE_ATOMICITY: u8 = 0x0A;
const FID_ASYNC_EVENT_CONFIG: u8 = 0x0B;

const LID_SUPPORTED_LOG_PAGES: u8 = 0x00;
const LID_ERROR_INFORMATION: u8 = 0x01;
const LID_SMART_HEALTH: u8 = 0x02;
const LID_FIRMWARE_SLOT: u8 = 0x03;
const LID_COMMAND_EFFECTS: u8 = 0x05;
const LID_FEATURE_EFFECTS: u8 = 0x12;
const EFFECTS_LIST_BYTES: usize = 256 * 4;
const SUPPORTED: u32 = 1;

/// The only namespace: its id and the number of namespaces (`NN`).
pub(super) const NAMESPACE_ID: u32 = 1;
/// Concurrent `Asynchronous Event Request` commands (`AERL` = 3, 0's based).
const AER_LIMIT: usize = 4;
/// Error Information log entries kept (`ELPE` = 63, 0's based).
const ERROR_LOG_ENTRIES: usize = 64;
const ERROR_LOG_ENTRY_BYTES: usize = 64;
/// Composite temperature thresholds and the reported temperature, in kelvin.
const WARNING_TEMPERATURE_K: u16 = 343;
const CRITICAL_TEMPERATURE_K: u16 = 358;
const COMPOSITE_TEMPERATURE_K: u16 = 300;
/// Largest transfer of one command, `MDTS` = 8 (2^8 pages of 4 KiB).
pub(super) const MDTS: u8 = 8;
const LBA_DATA_SIZE_LOG2: u8 = 9;

const EVENT_TYPE_ERROR: u8 = 0;
const EVENT_INFO_INVALID_DOORBELL_REGISTER: u8 = 0;
const EVENT_INFO_INVALID_DOORBELL_VALUE: u8 = 1;

/// Current values of the supported features (reset by a controller reset).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Features {
    arbitration: u32,
    temperature_over: u16,
    temperature_under: u16,
    error_recovery_tler: u16,
    /// `(NSQA, NCQA)`, 0's based, once the host has set them.
    queues: Option<(u16, u16)>,
    interrupt_coalescing: u16,
    interrupt_vector_disable: bool,
    write_atomicity: u32,
    async_event_config: u32,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            arbitration: 0x7,
            temperature_over: WARNING_TEMPERATURE_K,
            temperature_under: 0,
            error_recovery_tler: 0,
            queues: None,
            interrupt_coalescing: 0,
            interrupt_vector_disable: false,
            write_atomicity: 0,
            async_event_config: 0,
        }
    }
}

impl Features {
    /// `(NSQA, NCQA)` the controller currently offers, 0's based.
    fn allocated_queues(&self) -> (u16, u16) {
        self.queues
            .unwrap_or((MAX_IO_QUEUES as u16 - 1, MAX_IO_QUEUES as u16 - 1))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ErrorEntry {
    count: u64,
    sqid: u16,
    cid: u16,
    status_field: u16,
    nsid: u32,
}

/// Error Information log: newest entry first, `ELPE + 1` entries at most.
#[derive(Debug, Default)]
pub(super) struct ErrorLog {
    entries: VecDeque<ErrorEntry>,
    /// Total errors logged since power on; the log's `Error Count`.
    count: u64,
}

impl ErrorLog {
    fn push(&mut self, sqid: u16, cid: u16, status_field: u16, nsid: u32) {
        self.count += 1;
        self.entries.push_front(ErrorEntry {
            count: self.count,
            sqid,
            cid,
            status_field,
            nsid,
        });
        self.entries.truncate(ERROR_LOG_ENTRIES);
    }

    fn page(&self) -> Vec<u8> {
        let mut page = vec![0u8; ERROR_LOG_ENTRIES * ERROR_LOG_ENTRY_BYTES];
        for (index, entry) in self.entries.iter().enumerate() {
            let raw = &mut page[index * ERROR_LOG_ENTRY_BYTES..][..ERROR_LOG_ENTRY_BYTES];
            raw[0..8].copy_from_slice(&entry.count.to_le_bytes());
            raw[8..10].copy_from_slice(&entry.sqid.to_le_bytes());
            raw[10..12].copy_from_slice(&entry.cid.to_le_bytes());
            raw[12..14].copy_from_slice(&entry.status_field.to_le_bytes());
            // Parameter Error Location: not tracked, 0xFFFF.
            raw[14..16].copy_from_slice(&0xFFFFu16.to_le_bytes());
            raw[24..28].copy_from_slice(&entry.nsid.to_le_bytes());
            // Transport Type 0 = no transport specific information (PCIe has none here).
        }
        page
    }
}

/// Outstanding `Asynchronous Event Request` commands and queued events.
#[derive(Debug, Default)]
pub(super) struct AerState {
    outstanding: VecDeque<u16>,
    pending: VecDeque<u32>,
    ready: VecDeque<(u16, u32)>,
    /// One bit per asynchronous event type: set after an event of that type
    /// was reported, cleared by reading its log page with `RAE = 0`.
    masked: u8,
}

impl AerState {
    fn raise(&mut self, event_type: u8, info: u8, log_page: u8) {
        if self.masked & (1 << event_type) != 0 {
            return;
        }
        self.masked |= 1 << event_type;
        let dw0 = u32::from(event_type) | (u32::from(info) << 8) | (u32::from(log_page) << 16);
        match self.outstanding.pop_front() {
            Some(cid) => self.ready.push_back((cid, dw0)),
            None => self.pending.push_back(dw0),
        }
    }

    fn request(&mut self, cid: u16) -> Outcome {
        if self.outstanding.len() + self.ready.len() >= AER_LIMIT {
            return Outcome::Done(Completion::error(Status::AER_LIMIT_EXCEEDED));
        }
        match self.pending.pop_front() {
            Some(dw0) => Outcome::Done(Completion::ok(dw0)),
            None => {
                self.outstanding.push_back(cid);
                Outcome::Deferred
            }
        }
    }

    pub(super) fn next_ready(&self) -> Option<(u16, u32)> {
        self.ready.front().copied()
    }

    pub(super) fn pop_ready(&mut self) {
        self.ready.pop_front();
    }
}

/// Records a failed command in the Error Information log.
pub(super) fn log_command_error(
    state: &mut NvmeState,
    sqid: u16,
    command: &Command,
    status: Status,
    phase: bool,
) {
    let nsid = if command.nsid == 0 {
        0xFFFF_FFFF
    } else {
        command.nsid
    };
    state
        .errors
        .push(sqid, command.cid, status.field() | u16::from(phase), nsid);
}

fn report_doorbell_error(state: &mut NvmeState, qid: u16, info: u8) {
    state
        .errors
        .push(qid, 0xFFFF, Status::INVALID_FIELD.field(), 0xFFFF_FFFF);
    state
        .aer
        .raise(EVENT_TYPE_ERROR, info, LID_ERROR_INFORMATION);
}

/// A doorbell of a queue that does not exist (event "Write to Invalid Doorbell Register").
pub(super) fn report_invalid_doorbell_register(state: &mut NvmeState, qid: u16) {
    report_doorbell_error(state, qid, EVENT_INFO_INVALID_DOORBELL_REGISTER);
}

/// A doorbell value not below the queue size (event "Invalid Doorbell Write Value").
pub(super) fn report_invalid_doorbell_value(state: &mut NvmeState, qid: u16) {
    report_doorbell_error(state, qid, EVENT_INFO_INVALID_DOORBELL_VALUE);
}

/// Executes one admin command. Unsupported opcodes (including the optional
/// commands this controller does not implement) complete with `Invalid
/// Command Opcode`.
fn admin_opcode_supported(opcode: u8) -> bool {
    matches!(
        opcode,
        OPC_DELETE_SQ
            | OPC_CREATE_SQ
            | OPC_GET_LOG_PAGE
            | OPC_DELETE_CQ
            | OPC_CREATE_CQ
            | OPC_IDENTIFY
            | OPC_ABORT
            | OPC_SET_FEATURES
            | OPC_GET_FEATURES
            | OPC_ASYNC_EVENT_REQUEST
    )
}

pub(super) fn execute(
    state: &mut NvmeState,
    command: &Command,
    dma: &mut dyn GuestDma,
    report: &mut NvmeServiceReport,
) -> Outcome {
    if !admin_opcode_supported(command.opcode) {
        return Outcome::Done(Completion::error(Status::INVALID_OPCODE));
    }
    // Fused operations and SGLs are not supported (ONCS.FUSES = 0, SGLS = 0).
    if command.fuse != 0 || command.psdt != 0 {
        return Outcome::Done(Completion::error(Status::INVALID_FIELD));
    }
    if requires_zero_nsid(command) && command.nsid != 0 {
        return Outcome::Done(Completion::error(Status::INVALID_FIELD));
    }
    let result = match command.opcode {
        OPC_DELETE_SQ => delete_sq(state, command, dma, report),
        OPC_CREATE_SQ => create_sq(state, command),
        OPC_GET_LOG_PAGE => get_log_page(state, command, dma),
        OPC_DELETE_CQ => delete_cq(state, command),
        OPC_CREATE_CQ => create_cq(state, command),
        OPC_IDENTIFY => identify(state, command, dma),
        OPC_ABORT => abort(state, command),
        OPC_SET_FEATURES => set_features(state, command),
        OPC_GET_FEATURES => get_features(state, command),
        _ => return state.aer.request(command.cid),
    };
    Outcome::Done(result.unwrap_or_else(Completion::error))
}

/// NSID is required to be zero for controller-scoped admin commands. Error
/// Recovery is the one implemented feature here with namespace scope; its
/// existing namespace-or-all validation remains in `set_features`/`get_features`.
fn requires_zero_nsid(command: &Command) -> bool {
    match command.opcode {
        OPC_DELETE_SQ
        | OPC_CREATE_SQ
        | OPC_DELETE_CQ
        | OPC_CREATE_CQ
        | OPC_ABORT
        | OPC_ASYNC_EVENT_REQUEST => true,
        OPC_IDENTIFY => command.cdw10 & 0xFF == u32::from(CNS_CONTROLLER),
        OPC_SET_FEATURES | OPC_GET_FEATURES => (command.cdw10 & 0xFF) as u8 != FID_ERROR_RECOVERY,
        _ => false,
    }
}

/// Valid I/O queue id for creation: `1..=allocated + 1`. Submission and
/// completion queue ids are separate spaces; the caller checks that the id is free.
fn new_queue_id(command: &Command, allocated: u16) -> Result<usize, Status> {
    let qid = (command.cdw10 & 0xFFFF) as usize;
    if qid == 0 || qid > usize::from(allocated) + 1 || qid >= QUEUE_SLOTS {
        return Err(Status::INVALID_QUEUE_IDENTIFIER);
    }
    Ok(qid)
}

/// Queue size, contiguity and base alignment checks shared by both creators.
fn queue_geometry(command: &Command) -> Result<u16, Status> {
    let entries = (command.cdw10 >> 16) + 1;
    if !(2..=MAX_QUEUE_ENTRIES).contains(&entries) {
        return Err(Status::INVALID_QUEUE_SIZE);
    }
    // CAP.CQR = 1: queues must be physically contiguous (PC = 1).
    if command.cdw11 & 1 == 0 || !command.prp1.is_multiple_of(PAGE_BYTES) {
        return Err(Status::INVALID_FIELD);
    }
    Ok(entries as u16)
}

fn create_cq(state: &mut NvmeState, command: &Command) -> Result<Completion, Status> {
    let qid = new_queue_id(command, state.features.allocated_queues().1)?;
    if state.cqs[qid].is_some() {
        return Err(Status::INVALID_QUEUE_IDENTIFIER);
    }
    if !state.io_cq_entry_size_configured() {
        return Err(Status::INVALID_QUEUE_SIZE);
    }
    let entries = queue_geometry(command)?;
    let interrupt_enabled = command.cdw11 & 0b10 != 0;
    // Pin-based interrupts have a single vector, 0.
    if interrupt_enabled && command.cdw11 >> 16 != 0 {
        return Err(Status::INVALID_INTERRUPT_VECTOR);
    }
    state.cqs[qid] = Some(CompletionQueue::new(
        command.prp1,
        entries,
        interrupt_enabled,
    ));
    Ok(Completion::ok(0))
}

fn create_sq(state: &mut NvmeState, command: &Command) -> Result<Completion, Status> {
    let qid = new_queue_id(command, state.features.allocated_queues().0)?;
    if state.sqs[qid].is_some() {
        return Err(Status::INVALID_QUEUE_IDENTIFIER);
    }
    let cqid = (command.cdw11 >> 16) as usize;
    if cqid == 0 || cqid >= QUEUE_SLOTS || state.cqs[cqid].is_none() {
        return Err(Status::COMPLETION_QUEUE_INVALID);
    }
    if !state.io_sq_entry_size_configured() {
        return Err(Status::INVALID_QUEUE_SIZE);
    }
    let entries = queue_geometry(command)?;
    state.sqs[qid] = Some(SubmissionQueue::new(command.prp1, entries, cqid as u16));
    Ok(Completion::ok(0))
}

fn existing_io_queue_id(command: &Command) -> Result<usize, Status> {
    let qid = (command.cdw10 & 0xFFFF) as usize;
    if qid == 0 || qid >= QUEUE_SLOTS {
        return Err(Status::INVALID_QUEUE_IDENTIFIER);
    }
    Ok(qid)
}

fn delete_sq(
    state: &mut NvmeState,
    command: &Command,
    dma: &mut dyn GuestDma,
    report: &mut NvmeServiceReport,
) -> Result<Completion, Status> {
    let qid = existing_io_queue_id(command)?;
    if state.sqs[qid].is_none() {
        return Err(Status::INVALID_QUEUE_IDENTIFIER);
    }
    state.drain_io_queue_for_delete(qid, dma, report)?;
    state.sqs[qid] = None;
    Ok(Completion::ok(0))
}

fn delete_cq(state: &mut NvmeState, command: &Command) -> Result<Completion, Status> {
    let qid = existing_io_queue_id(command)?;
    if state.cqs[qid].is_none() {
        return Err(Status::INVALID_QUEUE_IDENTIFIER);
    }
    let in_use = state
        .sqs
        .iter()
        .skip(1)
        .flatten()
        .any(|sq| usize::from(sq.cqid) == qid);
    if in_use {
        return Err(Status::INVALID_QUEUE_DELETION);
    }
    state.cqs[qid] = None;
    Ok(Completion::ok(0))
}

/// Every command is executed to completion within the servicing call that
/// fetched it, so by the time an `Abort` runs its target has completed or was
/// never submitted: the command is not aborted (`CDW0` bit 0 = 1).
fn abort(state: &mut NvmeState, command: &Command) -> Result<Completion, Status> {
    let sqid = (command.cdw10 & 0xFFFF) as usize;
    if sqid >= QUEUE_SLOTS || state.sqs[sqid].is_none() {
        return Err(Status::INVALID_FIELD);
    }
    Ok(Completion::ok(1))
}

fn identify(
    state: &mut NvmeState,
    command: &Command,
    dma: &mut dyn GuestDma,
) -> Result<Completion, Status> {
    let namespace_valid = |nsid: u32| -> Result<(), Status> {
        if nsid == NAMESPACE_ID {
            Ok(())
        } else {
            Err(Status::INVALID_NAMESPACE)
        }
    };
    let data = match command.cdw10 & 0xFF {
        CNS_NAMESPACE => {
            namespace_valid(command.nsid)?;
            identify_namespace(state)
        }
        CNS_CONTROLLER => identify_controller(state),
        CNS_ACTIVE_NAMESPACES => {
            if command.nsid >= 0xFFFF_FFFE {
                return Err(Status::INVALID_NAMESPACE);
            }
            let mut list = vec![0u8; 4096];
            if command.nsid < NAMESPACE_ID {
                list[0..4].copy_from_slice(&NAMESPACE_ID.to_le_bytes());
            }
            list
        }
        CNS_NAMESPACE_DESCRIPTORS => {
            namespace_valid(command.nsid)?;
            namespace_descriptors(state)
        }
        _ => return Err(Status::INVALID_FIELD),
    };
    write_to_host(dma, command.prp1, command.prp2, &data)?;
    Ok(Completion::ok(0))
}

fn put(page: &mut [u8], offset: usize, bytes: &[u8]) {
    page[offset..offset + bytes.len()].copy_from_slice(bytes);
}

fn identify_controller(state: &NvmeState) -> Vec<u8> {
    let config = &state.config;
    let mut page = vec![0u8; 4096];
    // VID/SSVID: no PCI vendor id is assigned to the project (PCI binding S4-30).
    put(&mut page, 4, &config.serial);
    put(&mut page, 24, &config.model);
    put(&mut page, 64, &config.firmware_revision);
    page[72] = 0; // RAB: arbitration burst of one command
    page[77] = MDTS;
    put(&mut page, 80, &VERSION.to_le_bytes()); // VER
    page[111] = 1; // CNTRLTYPE: I/O controller
    page[258] = 3; // ACL: four concurrent Abort commands
    page[259] = (AER_LIMIT - 1) as u8; // AERL
    page[260] = 0x03; // FRMW: slot 1 read only, one slot
    page[261] = (1 << 2) | (1 << 1); // LPA: extended Get Log Page and command effects
    page[262] = (ERROR_LOG_ENTRIES - 1) as u8; // ELPE
    put(&mut page, 266, &WARNING_TEMPERATURE_K.to_le_bytes()); // WCTEMP
    put(&mut page, 268, &CRITICAL_TEMPERATURE_K.to_le_bytes()); // CCTEMP
    let capacity = u128::from(config.namespace_sectors) << LBA_DATA_SIZE_LOG2;
    put(&mut page, 280, &capacity.to_le_bytes()); // TNVMCAP; UNVMCAP stays 0
    let sqes = SQE_BYTES.ilog2() as u8;
    let cqes = CQE_BYTES.ilog2() as u8;
    page[512] = (sqes << 4) | sqes;
    page[513] = (cqes << 4) | cqes;
    put(&mut page, 516, &NAMESPACE_ID.to_le_bytes()); // NN
    // SUBNQN: nqn.2014-08.org.nvmexpress:uuid:<hyphenated UUID>, NUL padded.
    let uuid = config.subsystem_uuid;
    let mut nqn = String::from("nqn.2014-08.org.nvmexpress:uuid:");
    for (index, byte) in uuid.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            nqn.push('-');
        }
        nqn.push_str(&format!("{byte:02x}"));
    }
    put(&mut page, 768, nqn.as_bytes());
    // Power state descriptor 0 (NPSS = 0): 25.00 W maximum, no latencies.
    put(&mut page, 2048, &2500u16.to_le_bytes());
    page
}

fn identify_namespace(state: &NvmeState) -> Vec<u8> {
    let config = &state.config;
    let sectors = config.namespace_sectors.to_le_bytes();
    let mut page = vec![0u8; 4096];
    put(&mut page, 0, &sectors); // NSZE
    put(&mut page, 8, &sectors); // NCAP
    put(&mut page, 16, &sectors); // NUSE: no thin provisioning
    // NSFEAT, NLBAF (one format), FLBAS (format 0) are zero.
    let capacity = u128::from(config.namespace_sectors) << LBA_DATA_SIZE_LOG2;
    put(&mut page, 48, &capacity.to_le_bytes()); // NVMCAP
    put(&mut page, 104, &config.namespace_uuid); // NGUID
    // LBAF0: metadata size 0, LBADS = 9 (512-byte sectors), relative performance best.
    put(
        &mut page,
        128,
        &(u32::from(LBA_DATA_SIZE_LOG2) << 16).to_le_bytes(),
    );
    page
}

fn namespace_descriptors(state: &NvmeState) -> Vec<u8> {
    let mut page = vec![0u8; 4096];
    // NIDT 2 (NGUID), NIDL 16; then NIDT 4 (command set identifier, NVM = 0), NIDL 1.
    page[0] = 2;
    page[1] = 16;
    put(&mut page, 4, &state.config.namespace_uuid);
    page[20] = 4;
    page[21] = 1;
    page
}

fn get_log_page(
    state: &mut NvmeState,
    command: &Command,
    dma: &mut dyn GuestDma,
) -> Result<Completion, Status> {
    let lid = (command.cdw10 & 0xFF) as u8;
    let retain_event = command.cdw10 & (1 << 15) != 0;
    let dwords = (u64::from(command.cdw11 & 0xFFFF) << 16 | u64::from(command.cdw10 >> 16)) + 1;
    let length = dwords * 4;
    let offset = u64::from(command.cdw13) << 32 | u64::from(command.cdw12);
    if offset % 4 != 0 || command.cdw14 & (1 << 23) != 0 || length > PAGE_BYTES << MDTS {
        return Err(Status::INVALID_FIELD);
    }
    let discovery_log = matches!(
        lid,
        LID_SUPPORTED_LOG_PAGES | LID_COMMAND_EFFECTS | LID_FEATURE_EFFECTS
    );
    // CSI is ignored when CC.CSS selects the NVM command set directly.
    if discovery_log && command.cdw14 & 0x7F != 0 {
        return Err(Status::INVALID_FIELD);
    }
    // The supported logs are controller-wide (SMART per namespace is disabled).
    let controller_wide = matches!(command.nsid, 0 | 0xFFFF_FFFF);
    let mut page = match lid {
        LID_ERROR_INFORMATION if controller_wide => state.errors.page(),
        LID_SMART_HEALTH if controller_wide => smart_health_page(state),
        LID_FIRMWARE_SLOT if controller_wide => firmware_slot_page(state),
        LID_SUPPORTED_LOG_PAGES if controller_wide => supported_log_pages(),
        LID_COMMAND_EFFECTS if controller_wide => command_effects_page(),
        LID_FEATURE_EFFECTS if controller_wide => feature_effects_page(),
        LID_ERROR_INFORMATION
        | LID_SMART_HEALTH
        | LID_FIRMWARE_SLOT
        | LID_SUPPORTED_LOG_PAGES
        | LID_COMMAND_EFFECTS
        | LID_FEATURE_EFFECTS => return Err(Status::INVALID_FIELD),
        _ => return Err(Status::INVALID_LOG_PAGE),
    };
    if offset > page.len() as u64 {
        return Err(Status::INVALID_FIELD);
    }
    let end = offset + length;
    if end > page.len() as u64 {
        // The trailing dwords are undefined. Zero-fill the requested transfer
        // so the PRPs are interpreted using the original transfer length.
        page.resize(end as usize, 0);
    }
    write_to_host(
        dma,
        command.prp1,
        command.prp2,
        &page[offset as usize..end as usize],
    )?;
    if !retain_event {
        match lid {
            LID_ERROR_INFORMATION => state.aer.masked &= !(1 << EVENT_TYPE_ERROR),
            LID_SMART_HEALTH | LID_FIRMWARE_SLOT => state.aer.masked &= !(1 << 1),
            _ => {}
        }
    }
    Ok(Completion::ok(0))
}

fn supported_log_pages() -> Vec<u8> {
    let mut page = vec![0u8; EFFECTS_LIST_BYTES];
    for lid in [
        LID_SUPPORTED_LOG_PAGES,
        LID_ERROR_INFORMATION,
        LID_SMART_HEALTH,
        LID_FIRMWARE_SLOT,
        LID_COMMAND_EFFECTS,
        LID_FEATURE_EFFECTS,
    ] {
        put(&mut page, usize::from(lid) * 4, &SUPPORTED.to_le_bytes());
    }
    page
}

fn command_effects_page() -> Vec<u8> {
    let mut page = vec![0u8; 4096];
    for opcode in 0..=u8::MAX {
        if admin_opcode_supported(opcode) {
            put(&mut page, usize::from(opcode) * 4, &SUPPORTED.to_le_bytes());
        }
    }
    for (opcode, effects) in [
        (io::OPC_FLUSH, 0),
        (io::OPC_WRITE, 1 << 1),
        (io::OPC_READ, 0),
    ] {
        let offset = EFFECTS_LIST_BYTES + usize::from(opcode) * 4;
        put(&mut page, offset, &(SUPPORTED | effects).to_le_bytes());
    }
    page
}

fn feature_effects_page() -> Vec<u8> {
    const NAMESPACE_SCOPE: u32 = 1 << 20;
    const CONTROLLER_SCOPE: u32 = 1 << 21;
    let mut page = vec![0u8; EFFECTS_LIST_BYTES];
    for fid in [
        FID_ARBITRATION,
        FID_POWER_MANAGEMENT,
        FID_TEMPERATURE_THRESHOLD,
        FID_ERROR_RECOVERY,
        FID_NUMBER_OF_QUEUES,
        FID_INTERRUPT_COALESCING,
        FID_INTERRUPT_VECTOR_CONFIG,
        FID_WRITE_ATOMICITY,
        FID_ASYNC_EVENT_CONFIG,
    ] {
        let scope = if fid == FID_ERROR_RECOVERY {
            NAMESPACE_SCOPE
        } else {
            CONTROLLER_SCOPE
        };
        put(
            &mut page,
            usize::from(fid) * 4,
            &(SUPPORTED | scope).to_le_bytes(),
        );
    }
    page
}

fn smart_health_page(state: &NvmeState) -> Vec<u8> {
    let mut page = vec![0u8; 512];
    // Critical Warning (byte 0) = 0: no warning condition exists.
    put(&mut page, 1, &COMPOSITE_TEMPERATURE_K.to_le_bytes());
    page[3] = 100; // Available Spare
    page[4] = 10; // Available Spare Threshold
    // NOT VERIFIED against NVMe Base 2.4: data units are thousands of 512-byte
    // sectors, rounded up; busy time stays 0 (no time source).
    let io = &state.io;
    let units = |sectors: u64| u128::from(sectors.div_ceil(1000));
    put(&mut page, 32, &units(io.sectors_read).to_le_bytes()); // Data Units Read
    put(&mut page, 48, &units(io.sectors_written).to_le_bytes()); // Data Units Written
    put(&mut page, 64, &u128::from(io.read_commands).to_le_bytes()); // Host Read Commands
    put(&mut page, 80, &u128::from(io.write_commands).to_le_bytes()); // Host Write Commands
    put(&mut page, 112, &1u128.to_le_bytes()); // Power Cycles
    // Power On Hours stays 0: the device has no time source (documented gap).
    put(
        &mut page,
        176,
        &u128::from(state.errors.count).to_le_bytes(),
    );
    page
}

fn firmware_slot_page(state: &NvmeState) -> Vec<u8> {
    let mut page = vec![0u8; 512];
    page[0] = 0x01; // AFI: slot 1 active, no slot activates at the next reset
    put(&mut page, 8, &state.config.firmware_revision); // FRS1; slots 2-7 are empty
    page
}

fn set_features(state: &mut NvmeState, command: &Command) -> Result<Completion, Status> {
    // Save (bit 31) is not supported: no feature is persistent (ONCS bit 4 = 0).
    if command.cdw10 >> 31 != 0 {
        return Err(Status::INVALID_FIELD);
    }
    let value = command.cdw11;
    let features = &mut state.features;
    match (command.cdw10 & 0xFF) as u8 {
        FID_ARBITRATION => features.arbitration = value & 0xFFFF_FF07,
        FID_POWER_MANAGEMENT => {
            // NPSS = 0: only power state 0, no workload hint.
            if value != 0 {
                return Err(Status::INVALID_FIELD);
            }
        }
        FID_TEMPERATURE_THRESHOLD => {
            if temperature_selector(value)? {
                features.temperature_over = value as u16;
            } else {
                features.temperature_under = value as u16;
            }
        }
        FID_ERROR_RECOVERY => {
            namespace_or_all(command.nsid)?;
            // DULBE (bit 16) is not supported.
            if value >> 16 != 0 {
                return Err(Status::INVALID_FIELD);
            }
            features.error_recovery_tler = value as u16;
        }
        FID_NUMBER_OF_QUEUES => {
            let requested_sq = (value & 0xFFFF) as u16;
            let requested_cq = (value >> 16) as u16;
            if requested_sq == 0xFFFF || requested_cq == 0xFFFF {
                return Err(Status::INVALID_FIELD);
            }
            // The count can only change before any I/O queue exists.
            if state.sqs.iter().skip(1).any(Option::is_some)
                || state.cqs.iter().skip(1).any(Option::is_some)
            {
                return Err(Status::COMMAND_SEQUENCE_ERROR);
            }
            let limit = MAX_IO_QUEUES as u16 - 1;
            let granted = (requested_sq.min(limit), requested_cq.min(limit));
            features.queues = Some(granted);
            return Ok(Completion::ok(
                u32::from(granted.0) | (u32::from(granted.1) << 16),
            ));
        }
        FID_INTERRUPT_COALESCING => features.interrupt_coalescing = (value & 0xFFFF) as u16,
        FID_INTERRUPT_VECTOR_CONFIG => {
            // Pin-based interrupts have a single vector, 0.
            if value & 0xFFFF != 0 || value >> 17 != 0 {
                return Err(Status::INVALID_FIELD);
            }
            features.interrupt_vector_disable = value >> 16 != 0;
        }
        FID_WRITE_ATOMICITY => features.write_atomicity = value & 1,
        FID_ASYNC_EVENT_CONFIG => {
            // Only the SMART/Health critical-warning bits exist (OAES = 0).
            if value >> 8 != 0 {
                return Err(Status::INVALID_FIELD);
            }
            features.async_event_config = value;
        }
        _ => return Err(Status::INVALID_FIELD),
    }
    Ok(Completion::ok(0))
}

/// `TMPSEL` must name the composite sensor (0); returns whether the
/// over-temperature (`THSEL` = 0) or under-temperature (1) threshold is meant.
fn temperature_selector(value: u32) -> Result<bool, Status> {
    let select = ((value >> 16) & 0xF) as u8;
    let threshold = (value >> 20) & 0x3;
    if select != 0 || threshold > 1 {
        return Err(Status::INVALID_FIELD);
    }
    Ok(threshold == 0)
}

/// Namespace-specific feature: the only namespace, or all of them.
fn namespace_or_all(nsid: u32) -> Result<(), Status> {
    if matches!(nsid, 0 | NAMESPACE_ID | 0xFFFF_FFFF) {
        Ok(())
    } else {
        Err(Status::INVALID_NAMESPACE)
    }
}

fn get_features(state: &NvmeState, command: &Command) -> Result<Completion, Status> {
    // Select: 0 = current, 1 = default; saved and capabilities are not supported.
    let defaults = Features::default();
    let features = match (command.cdw10 >> 8) & 0x7 {
        0 => &state.features,
        1 => &defaults,
        _ => return Err(Status::INVALID_FIELD),
    };
    let value = command.cdw11;
    let dw0 = match (command.cdw10 & 0xFF) as u8 {
        FID_ARBITRATION => features.arbitration,
        FID_POWER_MANAGEMENT => 0,
        FID_TEMPERATURE_THRESHOLD => u32::from(if temperature_selector(value)? {
            features.temperature_over
        } else {
            features.temperature_under
        }),
        FID_ERROR_RECOVERY => {
            namespace_or_all(command.nsid)?;
            u32::from(features.error_recovery_tler)
        }
        FID_NUMBER_OF_QUEUES => {
            let (sq, cq) = features.allocated_queues();
            u32::from(sq) | (u32::from(cq) << 16)
        }
        FID_INTERRUPT_COALESCING => u32::from(features.interrupt_coalescing),
        FID_INTERRUPT_VECTOR_CONFIG => {
            if value & 0xFFFF != 0 {
                return Err(Status::INVALID_FIELD);
            }
            u32::from(features.interrupt_vector_disable) << 16
        }
        FID_WRITE_ATOMICITY => features.write_atomicity,
        FID_ASYNC_EVENT_CONFIG => features.async_event_config,
        _ => return Err(Status::INVALID_FIELD),
    };
    Ok(Completion::ok(dw0))
}
