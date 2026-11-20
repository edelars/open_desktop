//! End-to-end replay through the guest event log and the board's TPM CRB.

use super::*;
use crate::{InMemoryBackend, acpi_blob::sha256};
use std::path::{Path, PathBuf};

const CONFIG: u32 = 0x8000_0001;
const AUTHORITY: u32 = 0x8000_00e0;
const SEPARATOR: u32 = 4;
const HEADER_BYTES: usize = 65;
const EVENT_HEADER_BYTES: usize = 50;
const LOG_CAPACITY: usize = 256 * 1024;
const SCAN_BYTES: usize = 4 * 1024 * 1024;
const TPM_TEST_ENTROPY: [u8; 48] = [0x55; 48];
const EFI_GLOBAL_GUID: [u8; 16] = [
    0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c,
];
const DATABASE_GUID: [u8; 16] = [
    0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f,
];
const STOP: &[u8] =
    b"OD-FW no bootable option\r\nOD-FW STOP code=0x21 detail=0x0000000000000001\r\n";

#[derive(Debug, Eq, PartialEq)]
struct Event<'a> {
    kind: u32,
    name: String,
    value: &'a [u8],
}

#[test]
#[ignore = "requires a local ROM and fixtures from firmware_secure_boot_smoke.py --work"]
fn firmware_secure_boot_pcr7_replays_setup_enrollment_and_revocation() {
    let rom = std::fs::read(required_path("OD_SB_PCR7_ROM")).unwrap();
    let fixtures = required_path("OD_SB_PCR7_FIXTURES");
    let [
        (setup_log, setup_pcr),
        (enroll_log, enroll_pcr),
        (revoke_log, revoke_pcr),
    ] =
        std::thread::scope(|scope| {
            let rom = rom.as_slice();
            let fixtures = fixtures.as_path();
            [
            ("setup-1.nv", "unsigned.raw", b"OD-UEFI hello 0.1.0\r\n".as_slice()),
            (
                "setup-1.nv", "enroll.raw",
                b"OD-UEFI Secure Boot enrolled PK KEK db SetupMode=0 SecureBoot=1\r\n",
            ),
            (
                "enrolled-1.nv", "revoke.raw",
                b"OD-UEFI Secure Boot rejected unsigned at LoadImage and revoked at StartImage\r\n",
            ),
        ]
        .map(|(snapshot, disk, expected)| {
            scope.spawn(move || run(rom, fixtures, snapshot, disk, expected))
        })
        .map(|run| run.join().unwrap())
        });
    assert_ne!(setup_pcr, enroll_pcr);
    assert_ne!(enroll_pcr, revoke_pcr);
    assert_ne!(setup_pcr, revoke_pcr);

    let setup = replay_pcr7(&setup_log, setup_pcr);
    let enroll = replay_pcr7(&enroll_log, enroll_pcr);
    let revoke = replay_pcr7(&revoke_log, revoke_pcr);
    let initial_names = [
        "SecureBoot",
        "PK",
        "KEK",
        "db",
        "dbx",
        "AuditMode",
        "DeployedMode",
    ];
    assert_eq!((setup.len(), enroll.len(), revoke.len()), (8, 12, 10));
    assert_eq!(&setup[..], &enroll[..8]);
    for (name, event) in initial_names.iter().zip(&setup[..7]) {
        assert_eq!((event.kind, event.name.as_str()), (CONFIG, *name));
        let expected = if matches!(*name, "SecureBoot" | "AuditMode" | "DeployedMode") {
            &[0][..]
        } else {
            &[]
        };
        assert_eq!(event.value, expected);
    }
    assert_eq!(setup[7], revoke[7]);
    assert_eq!(setup[7].kind, SEPARATOR);
    assert_eq!(setup[7].value, [0; 4]);
    assert_eq!(
        enroll[10],
        Event {
            kind: CONFIG,
            name: "SecureBoot".into(),
            value: &[1]
        }
    );
    assert_eq!(revoke[0], enroll[10]);
    for (enrolled, initial, key) in [(8, 2, "KEK"), (9, 3, "db"), (11, 1, "PK")] {
        assert_eq!(
            (enroll[enrolled].kind, enroll[enrolled].name.as_str()),
            (CONFIG, key)
        );
        let expected = std::fs::read(fixtures.join(format!("{key}.esl"))).unwrap();
        assert!(!expected.is_empty());
        assert_eq!(enroll[enrolled].value, expected);
        assert_eq!(revoke[initial], enroll[enrolled]);
    }
    assert_eq!(&revoke[4..8], &setup[4..8]);
    // The fixture db contains one ESL with no header and one X.509 signature.
    assert_eq!(le32(enroll[9].value, 20), 0);
    assert_eq!(
        revoke[8],
        Event {
            kind: AUTHORITY,
            name: "db".into(),
            value: &enroll[9].value[28..],
        }
    );
    let expected_dbx = std::fs::read(fixtures.join("dbx.esl")).unwrap();
    assert_eq!(expected_dbx.len(), 76);
    assert_eq!(
        revoke[9],
        Event {
            kind: CONFIG,
            name: "dbx".into(),
            value: &expected_dbx,
        }
    );
}

fn required_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .unwrap_or_else(|| panic!("set {name} to run the PCR7 integration test"))
        .into()
}

fn run(
    rom: &[u8],
    fixtures: &Path,
    snapshot: &str,
    disk: &str,
    marker: &[u8],
) -> (Vec<u8>, [u8; 32]) {
    let config = FirmwareBoardConfig {
        acpi_pm: true,
        acpi_tables: true,
        nvme: Some(NvmeConfig::default()),
        nvram: Some(NvramConfig::Snapshot(
            std::fs::read(fixtures.join(snapshot)).unwrap(),
        )),
        tpm: Some(TpmConfig {
            entropy: TPM_TEST_ENTROPY.to_vec(),
            snapshot: None,
        }),
        ..FirmwareBoardConfig::default()
    };
    let mut board = load_firmware_image(rom, &config).unwrap();
    let bytes = std::fs::read(fixtures.join(disk)).unwrap();
    board
        .nvme()
        .unwrap()
        .attach_backend(Box::new(InMemoryBackend::from_bytes(bytes).unwrap()))
        .unwrap();
    let report = board.run_boot(300_000_000).unwrap();
    assert_eq!(report.exit, PagedProbeExit::Halted, "{disk}");
    assert!(
        report.com1.ends_with(&[marker, STOP].concat()),
        "{disk}: {}",
        String::from_utf8_lossy(&report.com1)
    );
    let bus = board.bus_mut();
    let log = locate_event_log(bus, config.ram_bytes);
    let pcr = read_pcr7(bus);
    println!(
        "{disk}: {} instructions, {} quanta",
        report.retired, report.quanta
    );
    (log, pcr)
}

fn locate_event_log(bus: &mut MachineBus, ram_bytes: u64) -> Vec<u8> {
    // The fixture board's boot pools allocate from the top of RAM. Identify
    // the unique primary log by its complete one-bank Spec ID header.
    let mut memory = vec![0u8; SCAN_BYTES];
    bus.read_flat_bytes(
        GuestPhysicalAddress::new(ram_bytes - SCAN_BYTES as u64),
        &mut memory,
    )
    .unwrap();
    let mut starts = memory
        .windows(HEADER_BYTES)
        .enumerate()
        .filter_map(|(offset, header)| {
            (header[..8] == [0, 0, 0, 0, 3, 0, 0, 0]
                && header[8..28] == [0; 20]
                && le32(header, 28) == 33
                && &header[32..48] == b"Spec ID Event03\0"
                && header[52..] == [0, 2, 106, 2, 1, 0, 0, 0, 11, 0, 32, 0, 0])
            .then_some(offset)
        });
    let start = starts.next().expect("primary TCG2 log");
    assert_eq!(starts.next(), None);
    memory[start..start + LOG_CAPACITY].to_vec()
}

fn read_pcr7(bus: &mut MachineBus) -> [u8; 32] {
    let addr = |offset| GuestPhysicalAddress::new(TPM_CRB_BASE + offset);
    for (offset, value) in [(8, 1), (0x40, 2), (0x40, 1)] {
        bus.write_physical(addr(offset), AccessWidth::Dword, value, |_| {})
            .unwrap();
    }
    let command: [u8; 20] = [
        0x80, 1, 0, 0, 0, 20, 0, 0, 1, 0x7e, 0, 0, 0, 1, 0, 0x0b, 3, 0x80, 0, 0,
    ];
    for (index, byte) in command.into_iter().enumerate() {
        bus.write_physical(
            addr(0x80 + index as u64),
            AccessWidth::Byte,
            u64::from(byte),
            |_| {},
        )
        .unwrap();
    }
    bus.write_physical(addr(0x4c), AccessWidth::Dword, 1, |_| {})
        .unwrap();
    let response: [u8; 62] = std::array::from_fn(|index| {
        bus.read_physical(addr(0x80 + index as u64), AccessWidth::Byte)
            .unwrap() as u8
    });
    assert_eq!(response[..10], [0x80, 1, 0, 0, 0, 62, 0, 0, 0, 0]);
    assert_eq!(
        response[14..30],
        [0, 0, 0, 1, 0, 0x0b, 3, 0x80, 0, 0, 0, 0, 0, 1, 0, 32]
    );
    response[30..].try_into().unwrap()
}

fn replay_pcr7(log: &[u8], actual: [u8; 32]) -> Vec<Event<'_>> {
    let mut offset = HEADER_BYTES;
    let mut pcr = [0; 32];
    let mut events = Vec::new();
    loop {
        let start = offset + EVENT_HEADER_BYTES;
        let header = &log[offset..start];
        if header == [0; EVENT_HEADER_BYTES] {
            break;
        }
        assert_eq!(le32(header, 8), 1);
        assert_eq!(header[12..14], [11, 0]);
        offset = start + le32(header, 46) as usize;
        if le32(header, 0) != 7 {
            continue;
        }
        let payload = &log[start..offset];
        let digest = sha256(payload);
        assert_eq!(header[14..46], digest);
        let mut pair = [0; 64];
        pair[..32].copy_from_slice(&pcr);
        pair[32..].copy_from_slice(&digest);
        pcr = sha256(&pair);
        let kind = le32(header, 4);
        if kind == SEPARATOR {
            events.push(Event {
                kind,
                name: String::new(),
                value: payload,
            });
        } else {
            assert!(matches!(kind, CONFIG | AUTHORITY));
            let units = le64(payload, 16) as usize;
            let end = 32 + 2 * units;
            let name = String::from_utf16(
                &payload[32..end]
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes(pair.try_into().unwrap()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let namespace = if matches!(name.as_str(), "db" | "dbx") {
                DATABASE_GUID
            } else {
                EFI_GLOBAL_GUID
            };
            assert_eq!(payload[..16], namespace);
            let value = &payload[end..];
            assert_eq!(le64(payload, 24) as usize, value.len());
            events.push(Event { kind, name, value });
        }
    }
    assert_eq!(pcr, actual);
    events
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
