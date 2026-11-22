//! Headless S5 NVMe board exercise. No guest instructions are executed: this
//! command drives the assembled machine's MMIO bindings and RAM-backed queues.

use od_host::persistent_nvme::attach_persistent_nvme_storage;
use od_host::storage_io::HostFileStorage;
use od_machine::{
    AccessWidth, FirmwareBoardConfig, GuestPhysicalAddress, NVME_MMIO_BASE, load_firmware_image,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const ASQ: u64 = 0x0100_0000;
const ACQ: u64 = 0x0100_1000;
const IDENTIFY: u64 = 0x0100_2000;
const IO_CQ: u64 = 0x0100_3000;
const IO_SQ: u64 = 0x0100_4000;
const IO_DATA: u64 = 0x0100_5000;
const SCRATCH_SECTORS: u64 = 8;
const SQE_BYTES: usize = 64;
const CQE_BYTES: usize = 16;

#[derive(Debug)]
struct Options {
    firmware: PathBuf,
    disk: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Report {
    namespace_sectors: u64,
    write_read_flush: bool,
    data_matches: bool,
    handoff_bytes: usize,
}

pub fn run(args: &[String]) -> ExitCode {
    let result = parse(args).and_then(|options| execute(&options.firmware, &options.disk));
    match result {
        Ok(report) => {
            println!(
                "{}",
                json!({
                    "schema_version": 1,
                    "command": "s5-nvme-stand",
                    "status": "passed",
                    "guest_executed": false,
                    "namespace_sectors": report.namespace_sectors,
                    "identify": true,
                    "queues": true,
                    "write_read_flush": report.write_read_flush,
                    "data_matches": report.data_matches,
                    "handoff_bytes": report.handoff_bytes,
                })
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!(
                "{}",
                json!({
                    "schema_version": 1,
                    "command": "s5-nvme-stand",
                    "status": "failed",
                    "guest_executed": false,
                    "error": error,
                })
            );
            ExitCode::from(5)
        }
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut firmware, mut disk) = (None, None);
    let mut index = 0;
    while index < args.len() {
        let target = match args[index].as_str() {
            "--image" => &mut firmware,
            "--disk" => &mut disk,
            other => return Err(format!("unknown argument '{other}'")),
        };
        index += 1;
        let value = args
            .get(index)
            .ok_or_else(|| format!("missing value after {}", args[index - 1]))?;
        *target = Some(PathBuf::from(value));
        index += 1;
    }
    Ok(Options {
        firmware: firmware.ok_or("--image is required")?,
        disk: disk.ok_or("--disk is required")?,
    })
}

fn execute(firmware_path: &Path, disk_path: &Path) -> Result<Report, String> {
    let image = fs::read(firmware_path).map_err(|error| format!("read firmware: {error}"))?;
    let scratch = HostFileStorage::create_new_exclusive(disk_path, SCRATCH_SECTORS * 512)
        .map_err(|error| format!("create new scratch RAW (path must not exist): {error:?}"))?;
    let config = FirmwareBoardConfig::s5_nvme_stand();
    let mut board = load_firmware_image(&image, &config)
        .map_err(|error| format!("load S5 board: {error:?}"))?;
    let nvme = board.nvme().ok_or("S5 profile did not assemble NVMe")?;
    attach_persistent_nvme_storage(&nvme, scratch)
        .map_err(|error| format!("attach persistent disk: {error:?}"))?;
    drive_nvme(&mut board, &nvme)
}

pub(crate) fn drive_nvme(
    board: &mut od_machine::LoadedFirmwareBoard,
    nvme: &od_machine::NvmeHandle,
) -> Result<Report, String> {
    let bus = board.bus_mut();
    write_mmio(bus, 0x24, AccessWidth::Dword, 0x000f_000f)?;
    write_mmio(bus, 0x28, AccessWidth::Qword, ASQ)?;
    write_mmio(bus, 0x30, AccessWidth::Qword, ACQ)?;
    write_mmio(bus, 0x14, AccessWidth::Dword, 0x0046_0001)?;
    if !nvme.is_ready() {
        return Err("controller did not become ready".into());
    }

    let mut admin_tail = 0u32;
    let mut admin_head = 0u32;
    let mut cid = 1u16;
    let identify_controller = command(0x06, cid, 0, IDENTIFY, 1, 0, 0);
    complete(
        board,
        nvme,
        ASQ,
        ACQ,
        &mut admin_tail,
        &mut admin_head,
        0x1000,
        0x1004,
        16,
        identify_controller,
    )?;
    cid += 1;
    let identify_namespace = command(0x06, cid, 1, IDENTIFY, 0, 0, 0);
    let ns_cqe = complete(
        board,
        nvme,
        ASQ,
        ACQ,
        &mut admin_tail,
        &mut admin_head,
        0x1000,
        0x1004,
        16,
        identify_namespace,
    )?;
    let ns_bytes = read_bytes(board.bus_mut(), IDENTIFY, 4096)?;
    let namespace_sectors = u64::from_le_bytes(ns_bytes[..8].try_into().unwrap());
    if namespace_sectors == 0 {
        return Err(format!(
            "Identify Namespace returned zero sectors: {ns_cqe:#x}"
        ));
    }

    cid += 1;
    let create_cq = command(0x05, cid, 0, IO_CQ, 1 | (3 << 16), 1, 0);
    complete(
        board,
        nvme,
        ASQ,
        ACQ,
        &mut admin_tail,
        &mut admin_head,
        0x1000,
        0x1004,
        16,
        create_cq,
    )?;
    cid += 1;
    let create_sq = command(0x01, cid, 0, IO_SQ, 1 | (3 << 16), 1 | (1 << 16), 0);
    complete(
        board,
        nvme,
        ASQ,
        ACQ,
        &mut admin_tail,
        &mut admin_head,
        0x1000,
        0x1004,
        16,
        create_sq,
    )?;

    let pattern = write_pattern();
    write_bytes(board.bus_mut(), IO_DATA, &pattern)?;
    let write = command(0x01, 0x20, 1, IO_DATA, 0, 0, 0);
    let mut io_tail = 0u32;
    let mut io_head = 0u32;
    complete(
        board,
        nvme,
        IO_SQ,
        IO_CQ,
        &mut io_tail,
        &mut io_head,
        0x1008,
        0x100c,
        4,
        write,
    )?;
    write_bytes(board.bus_mut(), IO_DATA, &[0; 512])?;
    let read = command(0x02, 0x21, 1, IO_DATA, 0, 0, 0);
    complete(
        board,
        nvme,
        IO_SQ,
        IO_CQ,
        &mut io_tail,
        &mut io_head,
        0x1008,
        0x100c,
        4,
        read,
    )?;
    let data_matches = read_bytes(board.bus_mut(), IO_DATA, 512)? == pattern;
    if !data_matches {
        return Err("one-sector read differs from the durable write".into());
    }
    let flush = command(0x00, 0x22, 1, 0, 0, 0, 0);
    complete(
        board,
        nvme,
        IO_SQ,
        IO_CQ,
        &mut io_tail,
        &mut io_head,
        0x1008,
        0x100c,
        4,
        flush,
    )?;

    Ok(Report {
        namespace_sectors,
        write_read_flush: true,
        data_matches,
        handoff_bytes: board.handoff_block().len(),
    })
}

/// The sector written to LBA 0 by [`drive_nvme`].
pub(crate) fn write_pattern() -> Vec<u8> {
    (0..512).map(|index| (index * 37 + 11) as u8).collect()
}

fn command(
    opcode: u8,
    cid: u16,
    nsid: u32,
    prp1: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
) -> [u8; SQE_BYTES] {
    let mut sqe = [0; SQE_BYTES];
    sqe[0] = opcode;
    sqe[2..4].copy_from_slice(&cid.to_le_bytes());
    sqe[4..8].copy_from_slice(&nsid.to_le_bytes());
    sqe[24..32].copy_from_slice(&prp1.to_le_bytes());
    sqe[40..44].copy_from_slice(&cdw10.to_le_bytes());
    sqe[44..48].copy_from_slice(&cdw11.to_le_bytes());
    sqe[48..52].copy_from_slice(&cdw12.to_le_bytes());
    sqe
}

#[allow(clippy::too_many_arguments)]
fn complete(
    board: &mut od_machine::LoadedFirmwareBoard,
    nvme: &od_machine::NvmeHandle,
    sq: u64,
    cq: u64,
    sq_tail: &mut u32,
    cq_head: &mut u32,
    sq_doorbell: u64,
    cq_doorbell: u64,
    queue_entries: u32,
    sqe: [u8; SQE_BYTES],
) -> Result<u32, String> {
    write_bytes(
        board.bus_mut(),
        sq + u64::from(*sq_tail) * SQE_BYTES as u64,
        &sqe,
    )?;
    *sq_tail = (*sq_tail + 1) % queue_entries;
    write_mmio(
        board.bus_mut(),
        sq_doorbell,
        AccessWidth::Dword,
        u64::from(*sq_tail),
    )?;
    if !nvme.doorbell_pending() {
        return Err("32-bit doorbell did not set doorbell_pending".into());
    }
    let report = nvme.service(board.bus_mut(), |_| {});
    if report.commands != 1 || report.completions != 1 || nvme.doorbell_pending() {
        return Err(format!(
            "controller service did not complete one command: {report:?}"
        ));
    }
    let cqe = read_u32(
        board.bus_mut(),
        cq + u64::from(*cq_head) * CQE_BYTES as u64 + 12,
    )?;
    let expected_cid = u16::from_le_bytes([sqe[2], sqe[3]]);
    let observed_cid = cqe as u16;
    if observed_cid != expected_cid {
        return Err(format!("completion CID {observed_cid} != {expected_cid}"));
    }
    if cqe >> 17 != 0 {
        return Err(format!(
            "NVMe command completed with status {:#x}",
            cqe >> 17
        ));
    }
    *cq_head = (*cq_head + 1) % queue_entries;
    write_mmio(
        board.bus_mut(),
        cq_doorbell,
        AccessWidth::Dword,
        u64::from(*cq_head),
    )?;
    Ok(cqe)
}

fn write_mmio(
    bus: &mut od_machine::MachineBus,
    offset: u64,
    width: AccessWidth,
    value: u64,
) -> Result<(), String> {
    bus.write_physical(
        GuestPhysicalAddress::new(NVME_MMIO_BASE + offset),
        width,
        value,
        |_| {},
    )
    .map_err(|error| format!("MMIO write at {offset:#x}: {error:?}"))
}

fn write_bytes(bus: &mut od_machine::MachineBus, address: u64, bytes: &[u8]) -> Result<(), String> {
    if !bytes.len().is_multiple_of(8) || address % 8 != 0 {
        return Err("DMA buffer writes must be qword aligned".into());
    }
    for (index, chunk) in bytes.chunks_exact(8).enumerate() {
        let value = u64::from_le_bytes(chunk.try_into().unwrap());
        bus.write_physical(
            GuestPhysicalAddress::new(address + (index * 8) as u64),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .map_err(|error| format!("RAM write: {error:?}"))?;
    }
    Ok(())
}

fn read_bytes(
    bus: &mut od_machine::MachineBus,
    address: u64,
    len: usize,
) -> Result<Vec<u8>, String> {
    if !len.is_multiple_of(8) || address % 8 != 0 {
        return Err("DMA buffer reads must be qword aligned".into());
    }
    let mut bytes = Vec::with_capacity(len);
    for index in 0..len / 8 {
        bytes.extend_from_slice(&read_u64(bus, address + (index * 8) as u64)?.to_le_bytes());
    }
    Ok(bytes)
}

fn read_u32(bus: &mut od_machine::MachineBus, address: u64) -> Result<u32, String> {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Dword)
        .map(|value| value as u32)
        .map_err(|error| format!("RAM read: {error:?}"))
}

fn read_u64(bus: &mut od_machine::MachineBus, address: u64) -> Result<u64, String> {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .map_err(|error| format!("RAM read: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_machine::{FIRMWARE_IMAGE_BYTES, FIRMWARE_LINK_BASE};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn persistent_nvme_round_trip_uses_the_assembled_stand_bus() {
        let dir = std::env::temp_dir().join(format!(
            "od-s5-nvme-stand-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let firmware = dir.join("stand.bin");
        let disk = dir.join("disk.raw");
        fs::write(&firmware, fixture_firmware()).unwrap();

        let result = execute(&firmware, &disk).unwrap();
        assert_eq!(result.namespace_sectors, 8);
        assert!(result.write_read_flush && result.data_matches);
        assert!(result.handoff_bytes > 0);
        let persisted = fs::read(&disk).unwrap();
        assert_eq!(persisted[0], 11);
        assert_eq!(persisted[1], 48);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn runner_refuses_an_existing_disk_path_without_modifying_it() {
        let dir = std::env::temp_dir().join(format!(
            "od-s5-nvme-existing-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let firmware = dir.join("stand.bin");
        let disk = dir.join("existing.raw");
        let original = vec![0xA5; 4096];
        fs::write(&firmware, fixture_firmware()).unwrap();
        fs::write(&disk, &original).unwrap();

        let error = execute(&firmware, &disk).unwrap_err();
        assert!(error.contains("path must not exist"));
        assert_eq!(fs::read(&disk).unwrap(), original);
        let _ = fs::remove_dir_all(dir);
    }

    fn fixture_firmware() -> Vec<u8> {
        let mut image = vec![0; FIRMWARE_IMAGE_BYTES];
        image[..8].copy_from_slice(b"ODFWIMG1");
        image[0x08..0x0c].copy_from_slice(&1u32.to_le_bytes());
        image[0x0c..0x10].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x10..0x18].copy_from_slice(&(FIRMWARE_IMAGE_BYTES as u64).to_le_bytes());
        image[0x18..0x20].copy_from_slice(&FIRMWARE_LINK_BASE.to_le_bytes());
        image[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        image[0x28..0x2a].copy_from_slice(&1u16.to_le_bytes());
        image[0x2a..0x2c].copy_from_slice(&1u16.to_le_bytes());
        image[0x40] = 0xf4;
        image[0x30..0x34].fill(0);
        let crc = crc32(&image);
        image[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
        image
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
