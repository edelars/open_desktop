//! I/O command set of the minimal NVMe controller (S7-16, slice B): `Read`,
//! `Write` and `Flush` over the attached [`NvmeBackend`].
//!
//! Opcodes Flush/Write/Read = 0x00/0x01/0x02 are cross-checked with Linux
//! `include/linux/nvme.h` (master, 2026-11-07), not with NVMe Base 2.4 / NVM
//! Command Set 1.3; the status values `LBA Out of Range` 0x80, `Namespace Not
//! Ready` 0x82, `Internal Error` 0x06, `Invalid Field` 0x2, `Data Transfer
//! Error` 0x4, `Invalid Namespace` 0xB and DNR 0x4000 are cross-checked with
//! `nvme.h` too. Everything else is `NOT VERIFIED against NVMe Base 2.4`:
//! the Read/Write dword layout used here (`CDW10/11` = SLBA, `CDW12[15:0]` =
//! NLB 0's based; `CDW12[31:16]` and `CDW13` hold the control, protection and
//! data-set-management fields: LR `CDW12[31]` and the DSM hints `CDW13[7:0]`
//! are ignored, the rest is rejected as `Invalid Field`), the DNR
//! values and the conditions under which the statuses are used, and the order
//! of the checks below. Contract: `docs/specs/NVME_DEVICE.md`, "Slice B".

use super::admin::{MDTS, NAMESPACE_ID};
use super::backend::SECTOR_BYTES;
use super::controller::NvmeState;
use super::queues::{
    Command, Completion, GuestDma, Outcome, PAGE_BYTES, Status, read_from_host, write_to_host,
};

const OPC_FLUSH: u8 = 0x00;
const OPC_WRITE: u8 = 0x01;
const OPC_READ: u8 = 0x02;

/// Largest transfer of one command in sectors: `MDTS` = 8 is 2^8 pages of 4 KiB.
const MAX_SECTORS: u64 = (PAGE_BYTES << MDTS) / SECTOR_BYTES as u64;

/// Counters behind the SMART / Health Information log; they survive a
/// controller reset like the error log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct IoStats {
    pub(super) read_commands: u64,
    pub(super) write_commands: u64,
    pub(super) sectors_read: u64,
    pub(super) sectors_written: u64,
}

/// Executes one I/O command. Unknown opcodes (including the optional NVM
/// commands the controller does not declare in `ONCS`) complete with
/// `Invalid Command Opcode`; every failure is a status, never a panic.
pub(super) fn execute(state: &mut NvmeState, command: &Command, dma: &mut dyn GuestDma) -> Outcome {
    let result = match command.opcode {
        OPC_FLUSH => flush(state, command),
        OPC_WRITE => write(state, command, dma),
        OPC_READ => read(state, command, dma),
        _ => Err(Status::INVALID_OPCODE),
    };
    Outcome::Done(result.unwrap_or_else(Completion::error))
}

/// Checks common to the three commands: no fused operation, no SGL, NSID 1,
/// backend attached (otherwise `Namespace Not Ready`).
fn check_common(state: &NvmeState, command: &Command) -> Result<(), Status> {
    if command.fuse != 0 || command.psdt != 0 {
        return Err(Status::INVALID_FIELD);
    }
    if command.nsid != NAMESPACE_ID {
        return Err(Status::INVALID_NAMESPACE);
    }
    if state.backend.is_none() {
        return Err(Status::NAMESPACE_NOT_READY);
    }
    Ok(())
}

/// Limited Retry bit of Read/Write (CDW12[31]): a hint, ignored.
const CDW12_LR: u32 = 1 << 31;
/// Dataset management hints of Read/Write (CDW13[7:0]): hints, ignored.
const CDW13_DSM_MASK: u32 = 0xFF;

/// A validated Read/Write range: first sector and number of sectors.
struct Transfer {
    lba: u64,
    sectors: u64,
}

impl Transfer {
    fn bytes(&self) -> usize {
        (self.sectors as usize) * SECTOR_BYTES
    }
}

/// Decodes and validates `SLBA`/`NLB` of `Read` and `Write`.
fn check_transfer(state: &NvmeState, command: &Command) -> Result<Transfer, Status> {
    check_common(state, command)?;
    // LR (CDW12[31]) and the dataset management hints (CDW13[7:0]) are only
    // hints: there are no retries and no access classes here, so they are
    // silently ignored (Linux sets them for REQ_FAILFAST_DEV | REQ_RAHEAD).
    // FUA, PRINFO, STC, DTYPE (the rest of CDW12[30:16]) and CDW13[31:8]
    // (DSPEC and reserved bits) are not supported: Invalid Field.
    if (command.cdw12 & !CDW12_LR) >> 16 != 0 || command.cdw13 & !CDW13_DSM_MASK != 0 {
        return Err(Status::INVALID_FIELD);
    }
    let sectors = u64::from(command.cdw12 & 0xFFFF) + 1;
    if sectors > MAX_SECTORS {
        return Err(Status::INVALID_FIELD);
    }
    let lba = u64::from(command.cdw10) | (u64::from(command.cdw11) << 32);
    let in_range = lba
        .checked_add(sectors)
        .is_some_and(|end| end <= state.config.namespace_sectors);
    if !in_range {
        return Err(Status::LBA_OUT_OF_RANGE);
    }
    Ok(Transfer { lba, sectors })
}

fn flush(state: &mut NvmeState, command: &Command) -> Result<Completion, Status> {
    check_common(state, command)?;
    let backend = state.backend.as_mut().ok_or(Status::NAMESPACE_NOT_READY)?;
    backend.flush().map_err(|_| Status::INTERNAL_ERROR)?;
    Ok(Completion::ok(0))
}

fn read(
    state: &mut NvmeState,
    command: &Command,
    dma: &mut dyn GuestDma,
) -> Result<Completion, Status> {
    let transfer = check_transfer(state, command)?;
    let backend = state.backend.as_mut().ok_or(Status::NAMESPACE_NOT_READY)?;
    let mut data = vec![0u8; transfer.bytes()];
    backend
        .read_sectors(transfer.lba, &mut data)
        .map_err(|_| Status::INTERNAL_ERROR)?;
    write_to_host(dma, command.prp1, command.prp2, &data)?;
    state.io.read_commands += 1;
    state.io.sectors_read += transfer.sectors;
    Ok(Completion::ok(0))
}

fn write(
    state: &mut NvmeState,
    command: &Command,
    dma: &mut dyn GuestDma,
) -> Result<Completion, Status> {
    let transfer = check_transfer(state, command)?;
    // The whole payload is fetched before the backend is touched: a bad PRP
    // leaves the disk unchanged.
    let data = read_from_host(dma, command.prp1, command.prp2, transfer.bytes())?;
    let backend = state.backend.as_mut().ok_or(Status::NAMESPACE_NOT_READY)?;
    backend
        .write_sectors(transfer.lba, &data)
        .map_err(|_| Status::INTERNAL_ERROR)?;
    state.io.write_commands += 1;
    state.io.sectors_written += transfer.sectors;
    Ok(Completion::ok(0))
}
