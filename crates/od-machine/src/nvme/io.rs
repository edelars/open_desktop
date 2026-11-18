//! I/O command set of the minimal NVMe controller (S7-16, slice B): `Read`,
//! `Write` and `Flush` over the attached [`NvmeBackend`].
//!
//! See `docs/specs/NVME_DEVICE.md` for the hardware profile and the status
//! values and boundaries verified against the NVMe specifications.

use super::admin::{MDTS, NAMESPACE_ID};
use super::backend::SECTOR_BYTES;
use super::controller::NvmeState;
use super::queues::{
    Command, Completion, GuestDma, Outcome, PAGE_BYTES, Status, read_from_host, write_to_host,
};

pub(super) const OPC_FLUSH: u8 = 0x00;
pub(super) const OPC_WRITE: u8 = 0x01;
pub(super) const OPC_READ: u8 = 0x02;

/// Largest transfer of one command in sectors: `MDTS` = 8 is 2^8 pages of 4 KiB.
const MAX_SECTORS: u64 = (PAGE_BYTES << MDTS) / SECTOR_BYTES as u64;
const NLB_MASK: u32 = 0xFFFF;
const LR_FUA_MASK: u32 = (1 << 31) | (1 << 30);
const WRITE_DTYPE_MASK: u32 = 0xF << 20;
const WRITE_DSPEC_MASK: u32 = 0xFFFF << 16;
const DSM_MASK: u32 = 0xFF;

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
    // FUA uses the same write-through and uncached read contract as plain I/O.
    // There is no extra retry or recovery path. DSM attributes are hints.
    // No Write directive is enabled, so its DTYPE and DSPEC are ignored.
    let common_control = NLB_MASK | LR_FUA_MASK;
    let (cdw12_mask, cdw13_mask, max_access_frequency) = if command.opcode == OPC_WRITE {
        (
            common_control | WRITE_DTYPE_MASK,
            DSM_MASK | WRITE_DSPEC_MASK,
            6,
        )
    } else {
        (common_control, DSM_MASK, 8)
    };
    if command.cdw12 & !cdw12_mask != 0
        || command.cdw13 & !cdw13_mask != 0
        || command.cdw13 & 0xF > max_access_frequency
    {
        return Err(Status::INVALID_FIELD);
    }
    let sectors = u64::from(command.cdw12 & NLB_MASK) + 1;
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
