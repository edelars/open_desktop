//! Frozen-image regression for the prototype CoW format (S9-22, "format
//! versions and migration").
//!
//! Compatibility table (hypothesis; there is no normative format yet, S9-40):
//!
//! | version | read | write | upgrade | downgrade |
//! |---------|------|-------|---------|-----------|
//! | 1 (prototype, `VERSION`) | yes | yes | n/a | n/a |
//! | unknown mandatory feature bit | refused (writable and read-only without a capability) | refused | n/a | n/a |
//! | any future version | not implemented | not implemented | not implemented | not implemented |
//!
//! There is a single prototype version, so migration (always into a new,
//! verified target, never in place) has nothing to migrate yet. What this file
//! provides is the base: a tiny frozen image of the current layout in
//! `tests/fixtures/cow/v1_created_committed.img` (one `create` plus one
//! commit) and a regression that opens it. Expectations are literals, written
//! down from the frozen bytes; they do not depend on the code that generated
//! the file. If the layout changes on purpose, this file must keep opening
//! (or be moved to an explicit upgrade path), never be regenerated silently.
//!
//! The image can be regenerated only deliberately with
//! `OD_WRITE_FIXTURE=1 cargo test -p od-storage write_fixture -- --ignored`.

use crate::cow_parse::{MANDATORY_AT, SLOT_CRC_AT};
use crate::cow_proto::{BLOCK_SIZE, CowError, CowFile, CowView, OpenClass, SLOT_SIZE, crc32};
use crate::io::CheckedStorage;
use crate::testing::fault::FaultHandle;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cow/v1_created_committed.img"
);

/// The one block committed into the fixture: `block[i] = (i * 7 + 3) mod 256`.
fn fixture_block() -> Vec<u8> {
    (0..BLOCK_SIZE).map(|i| (i * 7 + 3) as u8).collect()
}

fn load() -> Vec<u8> {
    std::fs::read(FIXTURE).expect("frozen fixture is checked in")
}

/// Bytes 0..68 of root slot 1 (generation 3) of the frozen image: magic,
/// version 1, block size 512, generation 3, one block, map at 1536, map CRC,
/// published end 2560, no features. Bytes 68..508 are zero.
const SLOT1_HEADER: [u8; 68] = [
    0x4f, 0x44, 0x43, 0x4f, 0x57, 0x50, 0x00, 0x01, // magic
    0x01, 0x00, 0x00, 0x00, // version 1
    0x00, 0x02, 0x00, 0x00, // block size 512
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // generation 3
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // block_count 1
    0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // map_offset 1536
    0x62, 0xdf, 0xb7, 0x77, // map CRC-32
    0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // end_offset 2560
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // mandatory features
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // optional features
];
const SLOT1_CRC: u32 = 0xda9b_8f04;
const FIXTURE_LEN: usize = 2560;

#[test]
fn frozen_image_layout_is_unchanged() {
    let bytes = load();
    assert_eq!(bytes.len(), FIXTURE_LEN);
    let slot1 = &bytes[SLOT_SIZE..2 * SLOT_SIZE];
    assert_eq!(&slot1[..68], &SLOT1_HEADER[..]);
    assert!(slot1[68..SLOT_CRC_AT].iter().all(|&b| b == 0));
    assert_eq!(
        u32::from_le_bytes(slot1[SLOT_CRC_AT..].try_into().expect("4 bytes")),
        SLOT1_CRC
    );
    assert_eq!(crc32(&slot1[..SLOT_CRC_AT]), SLOT1_CRC);
    // First data block starts right after the two slots.
    assert_eq!(&bytes[1024..1032], &[3, 10, 17, 24, 31, 38, 45, 52]);
}

fn assert_frozen_state(generation: u64, block_count: u64, block0: &[u8]) {
    assert_eq!(generation, 3);
    assert_eq!(block_count, 1);
    assert_eq!(&block0[..8], &[3, 10, 17, 24, 31, 38, 45, 52]);
    assert_eq!(block0.len(), BLOCK_SIZE);
    assert_eq!(block0[511], ((511usize * 7 + 3) % 256) as u8);
}

#[test]
fn frozen_image_opens_writable() {
    let handle = FaultHandle::new(&load(), 7);
    let (mut file, report) =
        CowFile::open(CheckedStorage::new(handle.open()).expect("caps")).expect("open");
    assert_eq!(report.class(), OpenClass::Committed);
    assert!(!report.possible_lost_commit);
    assert!(!report.rolled_forward);
    assert_eq!(report.tail_len, 0);
    assert_eq!(report.chosen_slot, 1);
    let block0 = file.read_block(0).expect("block 0");
    assert_frozen_state(file.generation(), file.block_count(), &block0);
    // The old image is still writable: a further commit moves to generation 4.
    file.commit(&[(0, vec![0xEE; BLOCK_SIZE].as_slice())])
        .expect("commit on old image");
    assert_eq!(file.generation(), 4);
}

#[test]
fn frozen_image_opens_read_only_salvage() {
    let handle = FaultHandle::new(&load(), 8);
    let (view, report) =
        CowView::salvage(CheckedStorage::new(handle.open()).expect("caps")).expect("salvage");
    assert_eq!(report.class(), OpenClass::Committed);
    assert!(!report.possible_lost_commit);
    let block0 = view.read_block(0).expect("block 0");
    assert_frozen_state(view.generation(), view.block_count(), &block0);
    // Salvage never writes.
    assert_eq!(handle.durable_bytes(), load());
}

#[test]
fn unknown_mandatory_feature_is_refused() {
    let mut bytes = load();
    // Bit 5 is not understood by this build.
    bytes[SLOT_SIZE + MANDATORY_AT] |= 1 << 5;
    let crc = crc32(&bytes[SLOT_SIZE..SLOT_SIZE + SLOT_CRC_AT]);
    bytes[SLOT_SIZE + SLOT_CRC_AT..2 * SLOT_SIZE].copy_from_slice(&crc.to_le_bytes());
    let handle = FaultHandle::new(&bytes, 9);
    let got = CowFile::open(CheckedStorage::new(handle.open()).expect("caps"));
    match got {
        Err(CowError::UnsupportedMandatoryFeature(5)) => {}
        Ok(_) => panic!("writable open must refuse an unknown mandatory feature"),
        Err(e) => panic!("unexpected refusal: {e:?}"),
    }
    // Without a capability for the bit, a read-only open refuses as well.
    let got = CowView::salvage(CheckedStorage::new(handle.open()).expect("caps"));
    assert!(matches!(got, Err(CowError::UnsupportedMandatoryFeature(5))));
    // A refusal never touches the media.
    assert_eq!(handle.durable_bytes(), bytes);
}

#[test]
#[ignore = "regenerates the frozen fixture; run only on purpose"]
fn write_fixture() {
    if std::env::var_os("OD_WRITE_FIXTURE").is_none() {
        return;
    }
    let handle = FaultHandle::new(&[], 1);
    let (mut file, _) =
        CowFile::create(CheckedStorage::new(handle.open()).expect("caps")).expect("create");
    file.commit(&[(0, fixture_block().as_slice())])
        .expect("commit");
    drop(file);
    std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().expect("dir")).expect("mkdir");
    std::fs::write(FIXTURE, handle.durable_bytes()).expect("write fixture");
}
