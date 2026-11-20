//! Native S7-28 failure probes, driven by tests/system/firmware_failure_smoke.py.

use super::*;
use crate::nvme::InMemoryBackend;
use std::path::Path;

const BUDGET: u64 = 20_000_000;
const NO_BOOT: &[u8] =
    b"OD-FW no bootable option\r\nOD-FW STOP code=0x21 detail=0x0000000000000001\r\n";
const MAPKEY_OK: &[u8] = b"OD-UEFI invalid MapKey rejected and valid key accepted\r\n";
const NV_VARS_OK: &[u8] = b"OD-UEFI vars defaults create read append enumerate delete verified\r\n";
const NV_RESTORED: &[u8] = b"OD-UEFI vars NV probe restored od-s7-10\r\n";
const NV_RESET: &[u8] = b"OD-FW S7-10 NVRAM discarded both invalid banks\r\n";

fn read(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn attach_disk(board: &mut LoadedFirmwareBoard, bytes: Vec<u8>) {
    board
        .nvme()
        .unwrap()
        .attach_backend(Box::new(InMemoryBackend::from_bytes(bytes).unwrap()))
        .unwrap();
}

fn boot(board: &mut LoadedFirmwareBoard, name: &str) -> FirmwareRunReport {
    let report = board
        .run_boot(BUDGET)
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    assert!(
        report.page_fault.is_none() && report.memory_error.is_none(),
        "{name}: {report:?}",
    );
    assert!(
        matches!(
            report.exit,
            PagedProbeExit::Halted | PagedProbeExit::Stopped
        ),
        "{name}: {:?} at {:#x}\n{}",
        report.exit,
        board.state().rip,
        String::from_utf8_lossy(&report.com1),
    );
    println!(
        "{name}: {} instructions, {} quanta",
        report.retired, report.quanta
    );
    report
}

fn ends_with(report: &FirmwareRunReport, expected: &[u8], name: &str) {
    assert_eq!(report.exit, PagedProbeExit::Halted, "{name}: {report:?}");
    assert!(
        report.com1.ends_with(expected),
        "{name}: {}",
        String::from_utf8_lossy(&report.com1)
    );
}

fn contains(report: &FirmwareRunReport, marker: &[u8]) -> bool {
    report
        .com1
        .windows(marker.len())
        .any(|bytes| bytes == marker)
}

fn boot_vars(rom: &[u8], nvram: NvramConfig, name: &str) -> (FirmwareRunReport, Vec<u8>) {
    let config = FirmwareBoardConfig {
        nvram: Some(nvram),
        fv_app: Some(FirmwareFvApp::Vars),
        ..FirmwareBoardConfig::default()
    };
    let mut board = load_firmware_image(rom, &config).unwrap();
    let report = boot(&mut board, name);
    ends_with(
        &report,
        b"OD-FW image vars: EFI_SUCCESS\r\nOD-FW STOP code=0x21 detail=0x0000000000000000\r\n",
        name,
    );
    assert!(contains(&report, NV_VARS_OK), "{name}: {report:?}");
    (report, board.nvram_snapshot().unwrap())
}

fn check_bad_banks(rom: &[u8]) {
    let (_, first) = boot_vars(rom, NvramConfig::Empty, "create legacy NV store");
    let (known, mut snapshot) = boot_vars(
        rom,
        NvramConfig::Snapshot(first.clone()),
        "update legacy NV store",
    );
    assert!(contains(&known, NV_RESTORED));
    let bank_bytes = (snapshot.len() - 16) / 2;
    let banks = [16, 16 + bank_bytes];
    let order = |image: &[u8]| {
        if le_u64(image, banks[0]).unwrap() < le_u64(image, banks[1]).unwrap() {
            banks
        } else {
            [banks[1], banks[0]]
        }
    };
    let [_, first_committed] = order(&first);
    let [older, newer] = order(&snapshot);
    assert!(le_u64(&snapshot, newer).unwrap() > le_u64(&first, first_committed).unwrap());
    // Use two completed boots. An intermediate commit can legitimately contain the probe's
    // work-in-progress variables and would not be a comparable starting store for the app.
    snapshot[older..older + bank_bytes]
        .copy_from_slice(&first[first_committed..first_committed + bank_bytes]);
    snapshot[newer + 12] ^= 0x01; // CRC32 of the newer bank.
    let (fallback, _) = boot_vars(
        rom,
        NvramConfig::Snapshot(snapshot.clone()),
        "legacy NVRAM newer bank CRC",
    );
    assert!(contains(&fallback, NV_RESTORED));
    assert!(!contains(&fallback, NV_RESET));

    snapshot[older + 12] ^= 0x01;
    assert!(matches!(
        NvramDevice::with_contents(&snapshot),
        Err(NvramContentsError::NoValidBank)
    ));
    let (empty, recovered) = boot_vars(
        rom,
        NvramConfig::Snapshot(snapshot),
        "legacy NVRAM both bank CRCs",
    );
    assert!(contains(&empty, NV_RESET));
    assert!(!contains(&empty, NV_RESTORED));
    assert!(contains(&empty, b"OD-UEFI vars NV probe set\r\n"));
    assert!(NvramDevice::with_contents(&recovered).is_ok());
}

fn remove_reset_register(board: &mut LoadedFirmwareBoard) {
    let mut handoff = board.handoff_block().to_vec();
    let device_table = le_u32(&handoff, 0x3c).unwrap() as usize;
    let entry = handoff[device_table..]
        .chunks_exact_mut(DEVICE_ENTRY_BYTES)
        .find(|entry| le_u32(entry, 0).unwrap() == DEVICE_CODE_ACPI_PM)
        .unwrap();
    entry[0x26..0x29].fill(0); // Upper half-word of param0 and reset value in param1.
    handoff[0x14..0x18].fill(0);
    let checksum = crc32_ieee(&[&handoff]);
    handoff[0x14..0x18].copy_from_slice(&checksum.to_le_bytes());
    for (offset, byte) in handoff.into_iter().enumerate() {
        board
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(FIRMWARE_HANDOFF_GPA + offset as u64),
                AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
    }
}

fn check_reset(rom: &[u8], directory: &Path, storage: &FirmwareBoardConfig) {
    let mut board = load_firmware_image(rom, storage).unwrap();
    attach_disk(&mut board, read(directory, "failure-probe.raw"));
    let report = boot(&mut board, "MapKey and ResetSystem without fixed hardware");
    assert!(
        contains(&report, MAPKEY_OK),
        "{}",
        String::from_utf8_lossy(&report.com1),
    );
    ends_with(
        &report,
        b"OD-FW ResetSystem has no fixed hardware\r\nOD-FW STOP code=0x22 detail=0x0000000000000001\r\n",
        "ResetSystem without fixed hardware",
    );
    drop(board);

    let mut board = load_firmware_image(
        rom,
        &FirmwareBoardConfig {
            acpi_tables: true,
            acpi_pm: true,
            ..storage.clone()
        },
    )
    .unwrap();
    remove_reset_register(&mut board);
    attach_disk(&mut board, read(directory, "failure-probe.raw"));
    let report = boot(&mut board, "ResetSystem without a reset register");
    assert!(
        contains(&report, MAPKEY_OK),
        "{}",
        String::from_utf8_lossy(&report.com1),
    );
    assert_eq!(report.exit, PagedProbeExit::Stopped, "{report:?}");
    assert_eq!(
        report.stop_reason,
        Some(MachineStopReason::Shutdown),
        "{report:?}"
    );
    let acpi = board.read_acpi_reservations().unwrap();
    let fadt = acpi
        .windows(276)
        .find(|table| &table[..8] == b"FACP\x14\x01\0\0")
        .expect("FADT present");
    assert_eq!(le_u32(fadt, 112).unwrap() & (1 << 10), 0);
    assert_eq!(&fadt[116..129], &[0; 13]);
    assert_eq!(fadt.iter().copied().fold(0u8, u8::wrapping_add), 0);
}

#[test]
#[ignore = "requires native UEFI ROM and fault fixtures from firmware_failure_smoke.py"]
fn firmware_failure_smoke_without_tpm_or_secure_boot() {
    let directory = std::env::var_os("OD_FW_FAILURE_FIXTURES").expect("OD_FW_FAILURE_FIXTURES");
    let directory = Path::new(&directory);
    let rom = read(directory, "firmware.bin");
    let apps_rom = read(directory, "firmware-apps.bin");

    let mut board = load_firmware_image(&rom, &FirmwareBoardConfig::default()).unwrap();
    board
        .bus_mut()
        .write_physical(
            GuestPhysicalAddress::new(FIRMWARE_HANDOFF_GPA + 8),
            AccessWidth::Word,
            2,
            |_| {},
        )
        .unwrap();
    let bad_version = boot(&mut board, "unknown handoff version");
    assert_eq!(
        bad_version.com1,
        b"OD-FW STOP code=0x03 detail=0x0000000000020000\r\n"
    );
    assert_eq!(bad_version.exit, PagedProbeExit::Halted);
    drop(board);

    check_bad_banks(&apps_rom);

    let mut board = load_firmware_image(&rom, &FirmwareBoardConfig::default()).unwrap();
    let absent = boot(&mut board, "no NVMe descriptor");
    assert_eq!(absent.exit, PagedProbeExit::Halted, "{absent:?}");
    assert!(contains(&absent, b"OD-FW no storage boot devices\r\n"));
    assert!(!contains(&absent, b"OD-FW STOP"));
    drop(board);

    let storage = FirmwareBoardConfig {
        nvme: Some(NvmeConfig::default()),
        nvram: Some(NvramConfig::Empty),
        ..FirmwareBoardConfig::default()
    };
    check_reset(&rom, directory, &storage);

    for (name, marker) in [
        ("healthy-esp.raw", b"OD-UEFI hello 0.1.0\r\n".as_slice()),
        ("bad-gpt.raw", b"".as_slice()),
        (
            "bad-pe.raw",
            b"OD-UEFI PE section rejected with EFI_LOAD_ERROR\r\n".as_slice(),
        ),
        (
            "fat-cycle.raw",
            b"OD-UEFI FAT chain rejected with EFI_VOLUME_CORRUPTED\r\n".as_slice(),
        ),
    ] {
        let mut board = load_firmware_image(&rom, &storage).unwrap();
        attach_disk(&mut board, read(directory, name));
        let report = boot(&mut board, name);
        ends_with(&report, NO_BOOT, name);
        if marker.is_empty() {
            assert!(!contains(&report, b"OD-UEFI hello 0.1.0\r\n"));
        } else {
            assert!(
                contains(&report, marker),
                "{name}: {}",
                String::from_utf8_lossy(&report.com1)
            );
        }
    }
}
