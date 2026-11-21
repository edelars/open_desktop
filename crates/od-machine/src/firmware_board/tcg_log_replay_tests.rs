//! ISSUE-630: replay of the whole TCG2 event log (Spec ID Event03) for PCR0-7 in both banks.
//!
//! `replay_log` extends every measured event of a crypto-agile log into SHA-256 and SHA-384
//! PCRs and the result is compared with `TPM2_PCR_Read` of the same boot. The always-run test
//! replays logs recorded from the guest (`tests/fixtures/firmware/tcg_log_*.bin`); the ignored
//! PCR7 integration test replays the logs of three live boards and can rewrite the fixtures.

use super::{secure_boot_pcr7_tests::le32, *};
use sha2::{Digest, Sha256, Sha384};

pub(super) const PCRS: usize = 8;
const HEADER_BYTES: usize = 69;
const EVENT_HEADER_BYTES: usize = 100;
const EV_NO_ACTION: u32 = 3;
const EV_SEPARATOR: u32 = 4;
const EV_ACTION: u32 = 5;
const EV_EFI_VARIABLE_DRIVER_CONFIG: u32 = 0x8000_0001;
const EV_EFI_ACTION: u32 = 0x8000_0007;
const EV_EFI_VARIABLE_AUTHORITY: u32 = 0x8000_00e0;
const TPM_ALG_SHA256: u16 = 0x000b;
const TPM_ALG_SHA384: u16 = 0x000c;
const FIXTURE_DIR_VARIABLE: &str = "OD_TCG_LOG_FIXTURE_DIR";

/// PCR0-7 of the SHA-256 bank and of the SHA-384 bank.
pub(super) type Banks = ([[u8; 32]; PCRS], [[u8; 48]; PCRS]);

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Replay {
    pub banks: Banks,
    /// Number of events extended into each PCR.
    pub extends: [usize; PCRS],
    /// `(offset, pcr, event type)` of every extended event.
    pub events: Vec<(usize, u32, u32)>,
    /// Octets from the start of the log to the first unused event slot.
    pub used: usize,
}

pub(super) fn replay_log(log: &[u8]) -> Result<Replay, String> {
    let header = log
        .get(..HEADER_BYTES)
        .ok_or("log is shorter than its Spec ID event")?;
    if header[..8] != [0, 0, 0, 0, 3, 0, 0, 0]
        || header[8..28] != [0; 20]
        || le32(header, 28) != 37
        || &header[32..48] != b"Spec ID Event03\0"
        || header[52..] != [0, 2, 106, 2, 2, 0, 0, 0, 11, 0, 32, 0, 12, 0, 48, 0, 0]
    {
        return Err("log does not start with the two-bank Spec ID Event03 header".into());
    }
    let mut replay = Replay {
        banks: ([[0; 32]; PCRS], [[0; 48]; PCRS]),
        extends: [0; PCRS],
        events: Vec::new(),
        used: 0,
    };
    let mut offset = HEADER_BYTES;
    loop {
        // A log trimmed to its used octets (the committed fixtures) ends without a zero slot.
        if offset == log.len() {
            replay.used = offset;
            return Ok(replay);
        }
        let header = log
            .get(offset..offset + EVENT_HEADER_BYTES)
            .ok_or_else(|| format!("event header at {offset} runs past the log"))?;
        if header.iter().all(|byte| *byte == 0) {
            replay.used = offset;
            return Ok(replay);
        }
        let (pcr, kind) = (le32(header, 0), le32(header, 4));
        if le32(header, 8) != 2 || header[12..14] != [11, 0] || header[46..48] != [12, 0] {
            return Err(format!(
                "event at {offset} is not a SHA-256 plus SHA-384 event"
            ));
        }
        let start = offset + EVENT_HEADER_BYTES;
        let data = start
            .checked_add(le32(header, 96) as usize)
            .and_then(|end| log.get(start..end))
            .ok_or_else(|| format!("event data at {start} runs past the log"))?;
        let (digest256, digest384) = (&header[14..46], &header[48..96]);
        let at = offset;
        offset = start + data.len();
        if kind == EV_NO_ACTION {
            continue;
        }
        let pcr_index = pcr as usize;
        if pcr_index >= PCRS {
            return Err(format!("event at {at} extends PCR{pcr}, outside PCR0-7"));
        }
        if digest_covers_data(kind)
            && (Sha256::digest(data)[..] != *digest256 || Sha384::digest(data)[..] != *digest384)
        {
            return Err(format!(
                "event at {at} (type {kind:#x}) digest is not the hash of its data"
            ));
        }
        let mut bank256 = Sha256::new();
        bank256.update(replay.banks.0[pcr_index]);
        bank256.update(digest256);
        replay.banks.0[pcr_index] = bank256.finalize().into();
        let mut bank384 = Sha384::new();
        bank384.update(replay.banks.1[pcr_index]);
        bank384.update(digest384);
        replay.banks.1[pcr_index] = bank384.finalize().into();
        replay.extends[pcr_index] += 1;
        replay.events.push((at, pcr, kind));
    }
}

/// For these types the TCG profile defines the digest as the hash of the event data.
const fn digest_covers_data(kind: u32) -> bool {
    matches!(
        kind,
        EV_SEPARATOR
            | EV_ACTION
            | EV_EFI_ACTION
            | EV_EFI_VARIABLE_DRIVER_CONFIG
            | EV_EFI_VARIABLE_AUTHORITY
    )
}

/// `TPM2_PCR_Read` of PCR0-7 for each bank through the board's CRB. One command returns at most
/// eight digests, so the two banks take two commands.
pub(super) fn read_pcrs(bus: &mut MachineBus) -> Banks {
    let bank256 = pcr_read(bus, TPM_ALG_SHA256, 32);
    let bank384 = pcr_read(bus, TPM_ALG_SHA384, 48);
    (
        std::array::from_fn(|pcr| bank256[pcr].as_slice().try_into().unwrap()),
        std::array::from_fn(|pcr| bank384[pcr].as_slice().try_into().unwrap()),
    )
}

fn pcr_read(bus: &mut MachineBus, algorithm: u16, digest_bytes: usize) -> Vec<Vec<u8>> {
    let addr = |offset| GuestPhysicalAddress::new(TPM_CRB_BASE + offset);
    for (offset, value) in [(8, 1), (0x40, 2), (0x40, 1)] {
        bus.write_physical(addr(offset), AccessWidth::Dword, value, |_| {})
            .unwrap();
    }
    let mut command = vec![0x80, 1, 0, 0, 0, 20, 0, 0, 1, 0x7e, 0, 0, 0, 1];
    command.extend(algorithm.to_be_bytes());
    command.extend([3, 0xff, 0, 0]);
    assert_eq!(command.len(), 20);
    for (index, byte) in command.iter().enumerate() {
        bus.write_physical(
            addr(0x80 + index as u64),
            AccessWidth::Byte,
            u64::from(*byte),
            |_| {},
        )
        .unwrap();
    }
    bus.write_physical(addr(0x4c), AccessWidth::Dword, 1, |_| {})
        .unwrap();
    let expected = 10 + 4 + 10 + 4 + PCRS * (2 + digest_bytes);
    let response: Vec<u8> = (0..expected)
        .map(|index| {
            bus.read_physical(addr(0x80 + index as u64), AccessWidth::Byte)
                .unwrap() as u8
        })
        .collect();
    assert_eq!(response[..2], [0x80, 1]);
    assert_eq!(response[2..6], (expected as u32).to_be_bytes());
    assert_eq!(response[6..10], [0; 4]);
    // pcrSelectionOut: one selection of this bank with PCR0-7 all returned.
    assert_eq!(response[14..18], [0, 0, 0, 1]);
    assert_eq!(response[18..20], algorithm.to_be_bytes());
    assert_eq!(response[20..24], [3, 0xff, 0, 0]);
    assert_eq!(response[24..28], [0, 0, 0, PCRS as u8]);
    let mut at = 28;
    (0..PCRS)
        .map(|_| {
            assert_eq!(response[at..at + 2], (digest_bytes as u16).to_be_bytes());
            at += 2 + digest_bytes;
            response[at - digest_bytes..at].to_vec()
        })
        .collect()
}

/// Fixture layout: SHA-256 PCR0-7 (8 x 32), SHA-384 PCR0-7 (8 x 48), then the used log octets.
pub(super) fn encode_fixture(banks: &Banks, log: &[u8]) -> Vec<u8> {
    let mut bytes = banks.0.concat();
    bytes.extend(banks.1.concat());
    bytes.extend(log);
    bytes
}

fn decode_fixture(bytes: &[u8]) -> (Banks, &[u8]) {
    let (bank256, rest) = bytes.split_at(PCRS * 32);
    let (bank384, log) = rest.split_at(PCRS * 48);
    (
        (
            std::array::from_fn(|pcr| bank256[pcr * 32..][..32].try_into().unwrap()),
            std::array::from_fn(|pcr| bank384[pcr * 48..][..48].try_into().unwrap()),
        ),
        log,
    )
}

/// Writes the raw boot (PCR0-7 and the whole log area) to `tcg_log_<name>.full` in
/// `OD_TCG_LOG_FIXTURE_DIR` before any check, so a failing replay can be examined offline.
pub(super) fn dump_boot(name: &str, log: &[u8], pcrs: &Banks) {
    if let Some(dir) = std::env::var_os(FIXTURE_DIR_VARIABLE) {
        let path = std::path::Path::new(&dir).join(format!("tcg_log_{name}.full"));
        std::fs::write(path, encode_fixture(pcrs, log)).unwrap();
    }
}

/// Replays `log` against the PCRs read from the same boot and, when `OD_TCG_LOG_FIXTURE_DIR` is
/// set, writes `tcg_log_<name>.bin` there so the always-run test can replay the same boot.
pub(super) fn check_boot(name: &str, log: &[u8], pcrs: &Banks) -> Replay {
    let replay = replay_log(log).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(
        &replay.banks, pcrs,
        "{name}: replayed PCR0-7 differ from TPM2_PCR_Read"
    );
    if let Some(dir) = std::env::var_os(FIXTURE_DIR_VARIABLE) {
        let path = std::path::Path::new(&dir).join(format!("tcg_log_{name}.bin"));
        std::fs::write(path, encode_fixture(pcrs, &log[..replay.used])).unwrap();
    }
    replay
}

const FIXTURES: [(&str, &[u8]); 3] = [
    (
        "unsigned",
        include_bytes!("../../../../tests/fixtures/firmware/tcg_log_unsigned.bin"),
    ),
    (
        "enroll",
        include_bytes!("../../../../tests/fixtures/firmware/tcg_log_enroll.bin"),
    ),
    (
        "revoke",
        include_bytes!("../../../../tests/fixtures/firmware/tcg_log_revoke.bin"),
    ),
];

#[test]
fn recorded_boot_logs_replay_to_the_recorded_pcr0_to_7_in_both_banks() {
    for (name, bytes) in FIXTURES {
        let (recorded, log) = decode_fixture(bytes);
        let replay = replay_log(log).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(replay.banks, recorded, "{name}");
        assert_eq!(
            replay.used,
            log.len(),
            "{name}: fixture keeps exactly the used log"
        );
        assert!(log.len() < 100 * 1024, "{name}");
        for pcr in 0..PCRS {
            // A PCR is non-zero exactly when the log extended it.
            assert_eq!(
                replay.extends[pcr] != 0,
                recorded.0[pcr] != [0; 32],
                "{name} PCR{pcr}"
            );
            assert_eq!(
                replay.extends[pcr] != 0,
                recorded.1[pcr] != [0; 48],
                "{name} PCR{pcr}"
            );
        }
        // Every boot of the fixture set measures into all of PCR0-7.
        assert!(
            replay.extends.iter().all(|&count| count > 0),
            "{name}: {:?}",
            replay.extends
        );
    }
}

#[test]
fn a_corrupted_log_no_longer_replays_to_the_recorded_pcrs() {
    let (recorded, log) = decode_fixture(FIXTURES[1].1);
    let replay = replay_log(log).unwrap();
    let (first, _, _) = *replay
        .events
        .iter()
        .find(|(_, _, kind)| !digest_covers_data(*kind))
        .expect("an event whose digest is not the hash of its data");
    let (last, _, _) = *replay.events.last().unwrap();
    // One flipped digest octet changes only the bank that holds it.
    for (offset, bank256_differs) in [(first + 14, true), (first + 48, false)] {
        let mut broken = log.to_vec();
        broken[offset] ^= 1;
        let outcome = replay_log(&broken).unwrap().banks;
        assert_eq!(
            (outcome.0 != recorded.0, outcome.1 != recorded.1),
            (bank256_differs, !bank256_differs)
        );
    }
    // A log that lost its last event no longer matches the PCRs.
    let mut truncated = log[..last].to_vec();
    truncated.resize(log.len(), 0);
    assert_ne!(replay_log(&truncated).unwrap().banks, recorded);
    // A log cut inside an event is rejected instead of replayed.
    assert!(replay_log(&log[..last + 10]).is_err());
    // A header that is not Spec ID Event03 is rejected.
    let mut headless = log.to_vec();
    headless[32] ^= 1;
    assert!(replay_log(&headless).is_err());
}
