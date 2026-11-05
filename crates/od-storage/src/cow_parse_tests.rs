//! Tests for the strict read-only parser in [`crate::cow_parse`].
//!
//! Expected bytes are specified here by hand and do not come from the encoder
//! the parser is meant to read: slots are laid out at literal offsets, and the
//! CRC-32 below is a separate bit-by-bit implementation (no table), so a
//! mistake shared between `cow_proto` and `cow_parse` cannot hide. The one test
//! that uses the prototype writer ([`valid_image_matches_independent_description`])
//! compares its output to the hand-built image byte for byte.

use crate::cow_parse::{
    Access, BlockDescriptor, Capabilities, Committed, Features, Limit, MAX_BLOCKS, MAX_FILE_BYTES,
    MAX_MAP_BYTES, ParseError, ParseKind, RootRecord, Section, Slot, Source, StorageSource,
    first_collision, parse_committed, parse_map, parse_slot_at, verify_block_data,
};
use crate::cow_proto::{CowFile, CowView};
use crate::io::{CheckedStorage, StorageError};
use crate::testing::fault::FaultHandle;
use crate::testing::memory::{Fault, MemoryStorage};
use crate::testing::rng::SplitMix64;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::ErrorKind;

// ------------------------------------------------------- independent oracle

/// CRC-32/ISO-HDLC, bit by bit: reflected polynomial 0xEDB88320, init and xorout
/// 0xFFFFFFFF.
fn ref_crc(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[test]
fn reference_crc_matches_published_check_value() {
    assert_eq!(ref_crc(b"123456789"), 0xCBF4_3926);
    assert_eq!(ref_crc(&[]), 0);
}

// ------------------------------------------------------------ hand fixtures

const MAGIC_BYTES: &[u8; 8] = b"ODCOWP\0\x01";
const FULL: Capabilities = Capabilities {
    read_mandatory: u64::MAX,
    write_mandatory: u64::MAX,
};
const CUR: Capabilities = Capabilities::CURRENT;

fn put_u32(img: &mut [u8], at: usize, v: u32) {
    img[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(img: &mut [u8], at: usize, v: u64) {
    img[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(img: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(img[at..at + 4].try_into().unwrap())
}

fn get_u64(img: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(img[at..at + 8].try_into().unwrap())
}

struct SlotSpec {
    generation: u64,
    block_count: u64,
    map_offset: u64,
    map_crc: u32,
    end_offset: u64,
}

/// Slot bytes at literal offsets: 0 magic, 8 version, 12 block size, 16
/// generation, 24 block count, 32 map offset, 40 map crc, 44 end offset, 52
/// mandatory, 60 optional, 68..508 zero, 508 crc.
fn slot_bytes(s: &SlotSpec) -> [u8; 512] {
    let mut b = [0u8; 512];
    b[0..8].copy_from_slice(MAGIC_BYTES);
    put_u32(&mut b, 8, 1);
    put_u32(&mut b, 12, 512);
    put_u64(&mut b, 16, s.generation);
    put_u64(&mut b, 24, s.block_count);
    put_u64(&mut b, 32, s.map_offset);
    put_u32(&mut b, 40, s.map_crc);
    put_u64(&mut b, 44, s.end_offset);
    let crc = ref_crc(&b[..508]);
    put_u32(&mut b, 508, crc);
    b
}

fn pat(i: usize) -> [u8; 512] {
    let mut b = [0u8; 512];
    for (j, x) in b.iter_mut().enumerate() {
        *x = (j as u8)
            .wrapping_mul(7)
            .wrapping_add((i as u8).wrapping_mul(31))
            .wrapping_add(1);
    }
    b
}

/// File layout of generation 2 with `n` blocks, written down independently of
/// the prototype: blocks from 1024, then the map, then the root copy on the
/// next 512 boundary; the file ends after the copy.
fn layout(n: u64) -> (u64, u64, u64, u64) {
    let map_off = 1024 + 512 * n;
    let map_end = map_off + 12 * n;
    let copy = map_end.div_ceil(512) * 512;
    (map_off, map_end, copy, copy + 512)
}

/// Generation 2 in slot 0, generation 1 (empty) in slot 1.
fn build(n: usize, block: impl Fn(usize) -> [u8; 512], crc_blocks: bool) -> Vec<u8> {
    let (map_off, _, copy, end) = layout(n as u64);
    let mut img = vec![0u8; end as usize];
    let mut map = Vec::new();
    for i in 0..n {
        let data = block(i);
        let phys = 1024 + 512 * i;
        img[phys..phys + 512].copy_from_slice(&data);
        map.extend_from_slice(&(phys as u64).to_le_bytes());
        map.extend_from_slice(&(if crc_blocks { ref_crc(&data) } else { 0 }).to_le_bytes());
    }
    img[map_off as usize..map_off as usize + map.len()].copy_from_slice(&map);
    let slot2 = slot_bytes(&SlotSpec {
        generation: 2,
        block_count: n as u64,
        map_offset: map_off,
        map_crc: ref_crc(&map),
        end_offset: end,
    });
    img[0..512].copy_from_slice(&slot2);
    img[copy as usize..copy as usize + 512].copy_from_slice(&slot2);
    img[512..1024].copy_from_slice(&slot_bytes(&SlotSpec {
        generation: 1,
        block_count: 0,
        map_offset: 1024,
        map_crc: 0,
        end_offset: 1024,
    }));
    img
}

// Layout of `baseline()` (3 blocks), spelled out as literals.
const MAP_OFF: usize = 2560;
const END: usize = 3584;

fn baseline() -> Vec<u8> {
    let img = build(3, pat, true);
    assert_eq!((layout(3).0, layout(3).3), (MAP_OFF as u64, END as u64));
    assert_eq!(img.len(), END);
    img
}

fn reseal_slot(img: &mut [u8], slot_off: usize) {
    let crc = ref_crc(&img[slot_off..slot_off + 508]);
    put_u32(img, slot_off + 508, crc);
}

/// Recomputes the map CRC of the slot (from its own fields) and the slot CRC.
fn reseal_map(img: &mut [u8], slot_off: usize) {
    let off = get_u64(img, slot_off + 32) as usize;
    let n = get_u64(img, slot_off + 24) as usize;
    let crc = ref_crc(&img[off..off + 12 * n]);
    put_u32(img, slot_off + 40, crc);
    reseal_slot(img, slot_off);
}

/// Overwrites a u64 slot field and fixes the slot CRC.
fn forge_u64(img: &mut [u8], slot_off: usize, field: usize, v: u64) {
    put_u64(img, slot_off + field, v);
    reseal_slot(img, slot_off);
}

fn forge_u32(img: &mut [u8], slot_off: usize, field: usize, v: u32) {
    put_u32(img, slot_off + field, v);
    reseal_slot(img, slot_off);
}

fn e(section: Section, offset: u64, kind: ParseKind) -> ParseError {
    ParseError {
        section,
        offset,
        kind,
    }
}

fn parse0(img: &[u8]) -> Result<Committed, ParseError> {
    parse_committed(img, 0, &CUR, Access::Writable)
}

// ------------------------------------------------------------------- (a)

#[test]
fn valid_image_matches_independent_description() {
    let img = baseline();
    let map_bytes = &img[MAP_OFF..MAP_OFF + 36];
    let map_crc = ref_crc(map_bytes);

    let got = parse0(&img).expect("valid image");
    assert_eq!(
        got.root,
        RootRecord {
            slot_offset: 0,
            generation: 2,
            block_count: 3,
            map_offset: 2560,
            map_crc,
            end_offset: 3584,
            features: Features::default(),
        }
    );
    let expected: Vec<BlockDescriptor> = (0..3)
        .map(|i| BlockDescriptor {
            phys: 1024 + 512 * i as u64,
            crc: ref_crc(&pat(i)),
        })
        .collect();
    assert_eq!(got.blocks, expected);
    for b in &got.blocks {
        verify_block_data(&img[..], b).expect("block data");
    }

    // The older slot is a complete, empty generation 1.
    let older = parse_committed(&img[..], 1, &CUR, Access::ReadOnly).expect("slot 1");
    assert_eq!(older.root.generation, 1);
    assert_eq!(older.root.block_count, 0);
    assert_eq!(older.root.end_offset, 1024);
    assert!(older.blocks.is_empty());

    // The root copy in the last sector is byte-identical to slot 0.
    assert_eq!(img[3072..3584], img[0..512]);
    let Slot::Root(copy) = parse_slot_at(&img[..], 3072, 0, &CUR, Access::ReadOnly).unwrap() else {
        panic!("copy is not a root");
    };
    assert_eq!(copy.slot_offset, 3072);
    assert_eq!(
        RootRecord {
            slot_offset: 0,
            ..copy
        },
        got.root
    );
}

/// What `create` followed by one commit of `n` blocks must leave on media,
/// written down independently of the prototype: slot 0 holds the empty
/// generation 2 and slot 1 the new generation 3, whose root copy ends the file.
fn build_created_then_committed(n: usize, block: impl Fn(usize) -> [u8; 512]) -> Vec<u8> {
    let mut img = build(n, block, true);
    // Re-seal as generation 3 in slot 1; slot 0 becomes the empty generation 2.
    let slot3 = {
        let mut s = img[0..512].to_vec();
        put_u64(&mut s, 16, 3);
        let crc = ref_crc(&s[..508]);
        put_u32(&mut s, 508, crc);
        s
    };
    let end = img.len();
    img[512..1024].copy_from_slice(&slot3);
    img[end - 512..].copy_from_slice(&slot3);
    img[0..512].copy_from_slice(&slot_bytes(&SlotSpec {
        generation: 2,
        block_count: 0,
        map_offset: 1024,
        map_crc: 0,
        end_offset: 1024,
    }));
    img
}

#[test]
fn prototype_writer_output_equals_hand_built_image() {
    let handle = FaultHandle::new(&[], 0x5909);
    let storage = CheckedStorage::new(handle.open()).expect("caps");
    let (mut file, _ack) = CowFile::create(storage).expect("create");
    // Created file: two empty roots, generation 2 in slot 0, 1 in slot 1.
    let mut created = vec![0u8; 1024];
    created[0..512].copy_from_slice(&slot_bytes(&SlotSpec {
        generation: 2,
        block_count: 0,
        map_offset: 1024,
        map_crc: 0,
        end_offset: 1024,
    }));
    created[512..1024].copy_from_slice(&slot_bytes(&SlotSpec {
        generation: 1,
        block_count: 0,
        map_offset: 1024,
        map_crc: 0,
        end_offset: 1024,
    }));
    assert_eq!(handle.durable_bytes(), created);

    let blocks: Vec<[u8; 512]> = (0..3).map(pat).collect();
    let writes: Vec<(u64, &[u8])> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (i as u64, &b[..]))
        .collect();
    let _ack = file.commit(&writes).expect("commit");
    let proto_bytes = handle.durable_bytes();
    assert_eq!(proto_bytes, build_created_then_committed(3, pat));

    // The prototype's own reader agrees with the strict parser.
    let parsed =
        parse_committed(&proto_bytes[..], 1, &CUR, Access::ReadOnly).expect("strict parse");
    assert_eq!(parsed.root.generation, 3);
    let (view, report) =
        CowView::salvage(CheckedStorage::new(handle.open()).expect("caps")).expect("salvage");
    assert_eq!(report.generation, parsed.root.generation);
    assert_eq!(view.block_count(), parsed.root.block_count);
    for (i, d) in parsed.blocks.iter().enumerate() {
        assert_eq!(view.block_offset(i as u64), Some(d.phys));
    }
}

#[test]
fn storage_source_parses_like_slice_and_reads_only() {
    let img = baseline();
    let storage = CheckedStorage::new(MemoryStorage::new_read_only(&img)).expect("caps");
    let via_storage = parse_committed(&StorageSource(&storage), 0, &CUR, Access::ReadOnly);
    assert_eq!(via_storage, parse0(&img));
    assert!(via_storage.is_ok());
}

#[test]
fn storage_failure_is_reported_as_io_at_the_read() {
    let img = baseline();
    let mut mem = MemoryStorage::new(&img);
    mem.push_fault(Fault::Error(ErrorKind::Other));
    let storage = CheckedStorage::new(mem).expect("caps");
    let err = parse_committed(&StorageSource(&storage), 0, &CUR, Access::ReadOnly).unwrap_err();
    assert_eq!((err.section, err.offset), (Section::Slot, 0));
    assert!(matches!(err.kind, ParseKind::Io(StorageError::Io { .. })));
}

// ------------------------------------------------------------------- (b)

#[test]
fn truncation_at_every_length_is_exact() {
    let img = baseline();
    for len in 0..=END + 70 {
        let cut = &img[..len.min(END)];
        // Bytes past the published end are an unreferenced tail.
        let mut padded = cut.to_vec();
        padded.resize(len, 0xA5);
        let got = parse0(&padded);
        let expected = if len < 512 {
            Err(e(
                Section::Slot,
                0,
                ParseKind::Truncated {
                    needed: 512,
                    available: len as u64,
                },
            ))
        } else if len < END {
            Err(e(
                Section::Slot,
                44,
                ParseKind::Truncated {
                    needed: END as u64,
                    available: len as u64,
                },
            ))
        } else {
            Ok(parse0(&img).unwrap())
        };
        assert_eq!(got, expected, "slot 0, length {len}");

        // Slot 1 is generation 1: a complete file once the header is there.
        let got1 = parse_committed(&padded[..], 1, &CUR, Access::ReadOnly);
        if len < 1024 {
            let e1 = e(
                Section::Slot,
                512,
                ParseKind::Truncated {
                    needed: 512,
                    available: (len as u64).saturating_sub(512),
                },
            );
            assert_eq!(got1, Err(e1), "slot 1, length {len}");
        } else {
            assert_eq!(got1.unwrap().root.generation, 1, "slot 1, length {len}");
        }
    }
}

#[test]
fn truncation_boundaries_named() {
    let img = baseline();
    // header end, inside block 0, block 0 end, map start/end, copy start, last byte
    for len in [1024, 1025, 1536, 2559, 2560, 2595, 2596, 3071, 3072, 3583] {
        let got = parse0(&img[..len]).unwrap_err();
        assert_eq!(
            got,
            e(
                Section::Slot,
                44,
                ParseKind::Truncated {
                    needed: 3584,
                    available: len as u64
                }
            ),
            "len {len}"
        );
    }
    // A block cut in the middle.
    let d = BlockDescriptor {
        phys: 1024,
        crc: ref_crc(&pat(0)),
    };
    assert_eq!(
        verify_block_data(&img[..1100], &d),
        Err(e(
            Section::Block,
            1024,
            ParseKind::Truncated {
                needed: 512,
                available: 76
            }
        ))
    );
}

// ------------------------------------------------------------------- (c)

#[test]
fn overflow_and_huge_values() {
    let base = baseline();
    let at = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut img = base.clone();
        f(&mut img);
        parse0(&img)
    };

    // block_count near u64::MAX and just above the limit.
    for count in [u64::MAX, u64::MAX - 1, MAX_BLOCKS + 1] {
        assert_eq!(
            at(&|i| forge_u64(i, 0, 24, count)),
            Err(e(
                Section::Slot,
                24,
                ParseKind::LimitExceeded(Limit::Blocks)
            )),
            "count {count}"
        );
    }
    // Exactly at the limit the slot is fine, but the map cannot fit.
    assert_eq!(
        at(&|i| forge_u64(i, 0, 24, MAX_BLOCKS)),
        Err(e(Section::Slot, 32, ParseKind::OutOfRange))
    );

    // end_offset.
    for end in [u64::MAX, MAX_FILE_BYTES + 1] {
        assert_eq!(
            at(&|i| forge_u64(i, 0, 44, end)),
            Err(e(
                Section::Slot,
                44,
                ParseKind::LimitExceeded(Limit::FileBytes)
            ))
        );
    }
    assert_eq!(
        at(&|i| forge_u64(i, 0, 44, MAX_FILE_BYTES)),
        Err(e(
            Section::Slot,
            44,
            ParseKind::Truncated {
                needed: MAX_FILE_BYTES,
                available: END as u64
            }
        ))
    );
    for end in [3585u64, 1023, 0, 512] {
        assert_eq!(
            at(&|i| forge_u64(i, 0, 44, end)),
            Err(e(Section::Slot, 44, ParseKind::OutOfRange)),
            "end {end}"
        );
    }

    // map_offset: sum overflows, aligned but far outside, unaligned, in the
    // header, running into the root copy.
    assert_eq!(
        at(&|i| forge_u64(i, 0, 32, u64::MAX - 5)),
        Err(e(Section::Slot, 32, ParseKind::Overflow))
    );
    for off in [u64::MAX - 511, 1 << 40, 2561, 512, 0, 3072] {
        assert_eq!(
            at(&|i| forge_u64(i, 0, 32, off)),
            Err(e(Section::Slot, 32, ParseKind::OutOfRange)),
            "map_offset {off}"
        );
    }

    // Entry arithmetic: phys + block size overflows; aligned but outside.
    let entry1 = (MAP_OFF + 12) as u64;
    assert_eq!(
        at(&|i| {
            put_u64(i, MAP_OFF + 12, u64::MAX - 100);
            reseal_map(i, 0);
        }),
        Err(e(Section::Block, entry1, ParseKind::Overflow))
    );
    assert_eq!(
        at(&|i| {
            put_u64(i, MAP_OFF + 12, u64::MAX - 1023);
            reseal_map(i, 0);
        }),
        Err(e(Section::Block, entry1, ParseKind::OutOfRange))
    );

    // Generation 0, wrong parity, u64::MAX (slot 1 is odd).
    assert_eq!(
        at(&|i| forge_u64(i, 0, 16, 0)),
        Err(e(Section::Slot, 16, ParseKind::Generation))
    );
    assert_eq!(
        at(&|i| forge_u64(i, 0, 16, 3)),
        Err(e(Section::Slot, 16, ParseKind::Generation))
    );
    let mut img = base.clone();
    forge_u64(&mut img, 512, 16, u64::MAX);
    assert_eq!(
        parse_committed(&img[..], 1, &CUR, Access::ReadOnly),
        Err(e(Section::Slot, 528, ParseKind::Generation))
    );
    forge_u64(&mut img, 512, 16, u64::MAX - 2);
    assert!(parse_committed(&img[..], 1, &CUR, Access::ReadOnly).is_ok());

    // Slot position arithmetic.
    assert_eq!(
        parse_slot_at(&base[..], u64::MAX - 100, 0, &CUR, Access::ReadOnly),
        Err(e(Section::Slot, u64::MAX - 100, ParseKind::Overflow))
    );
    assert_eq!(
        parse_slot_at(&base[..], u64::MAX / 2, 0, &CUR, Access::ReadOnly),
        Err(e(
            Section::Slot,
            u64::MAX / 2,
            ParseKind::Truncated {
                needed: 512,
                available: 0
            }
        ))
    );
    assert_eq!(
        parse_slot_at(&base[..], 0, 2, &CUR, Access::ReadOnly),
        Err(e(Section::Slot, 0, ParseKind::OutOfRange))
    );
    let d = BlockDescriptor {
        phys: u64::MAX - 10,
        crc: 0,
    };
    assert_eq!(
        verify_block_data(&base[..], &d),
        Err(e(Section::Block, u64::MAX - 10, ParseKind::Overflow))
    );
}

// ------------------------------------------------------------------- (d)

#[test]
fn duplicate_overlap_and_out_of_range_entries() {
    let base = baseline();
    let entry = |i: usize| (MAP_OFF + 12 * i) as u64;
    let set_phys = |img: &mut Vec<u8>, index: usize, phys: u64| {
        put_u64(img, MAP_OFF + 12 * index, phys);
        reseal_map(img, 0);
    };
    let run = |index: usize, phys: u64| {
        let mut img = base.clone();
        set_phys(&mut img, index, phys);
        parse0(&img)
    };

    // Two entries on one physical block: reported at the later entry.
    assert_eq!(
        run(2, 1024),
        Err(e(
            Section::Block,
            entry(2),
            ParseKind::Duplicate { first_index: 0 }
        ))
    );
    assert_eq!(
        run(2, 1536),
        Err(e(
            Section::Block,
            entry(2),
            ParseKind::Duplicate { first_index: 1 }
        ))
    );
    assert_eq!(
        run(0, 2048),
        Err(e(
            Section::Block,
            entry(2),
            ParseKind::Duplicate { first_index: 0 }
        ))
    );

    // A block on top of the map.
    assert_eq!(
        run(1, 2560),
        Err(e(
            Section::Block,
            entry(1),
            ParseKind::Overlap { other_offset: 2560 }
        ))
    );
    // Header, root copy, past the published area, unaligned.
    for (i, phys) in [
        (0, 512u64),
        (0, 0),
        (1, 3072),
        (2, 3584),
        (0, 1025),
        (0, 1023),
    ] {
        assert_eq!(
            run(i, phys),
            Err(e(Section::Block, entry(i), ParseKind::OutOfRange)),
            "entry {i} phys {phys}"
        );
    }

    // Map CRC mismatch (no reseal), located at the map start.
    let mut img = base.clone();
    img[MAP_OFF + 5] ^= 0x10;
    let stored = get_u32(&img, 40);
    assert_eq!(
        parse0(&img),
        Err(e(
            Section::Map,
            MAP_OFF as u64,
            ParseKind::Crc {
                stored,
                computed: ref_crc(&img[MAP_OFF..MAP_OFF + 36])
            }
        ))
    );
}

#[test]
fn collision_detection_without_alignment() {
    // Alignment makes partial overlap unreachable through `parse_map`, so the
    // collision rule is checked directly.
    assert_eq!(first_collision(&[]), None);
    assert_eq!(first_collision(&[(0, 512), (512, 512), (1024, 512)]), None);
    assert_eq!(first_collision(&[(0, 0), (0, 512)]), None);
    assert_eq!(
        first_collision(&[(1024, 512), (1500, 512)]),
        Some((1, ParseKind::Overlap { other_offset: 1024 }))
    );
    assert_eq!(
        first_collision(&[(1500, 512), (1024, 512)]),
        Some((1, ParseKind::Overlap { other_offset: 1500 }))
    );
    assert_eq!(
        first_collision(&[(0, 512), (4096, 512), (4096, 512), (4096, 512)]),
        Some((2, ParseKind::Duplicate { first_index: 1 }))
    );
    assert_eq!(
        first_collision(&[(10, u64::MAX - 20), (100, 1)]),
        Some((1, ParseKind::Overlap { other_offset: 10 }))
    );
}

// ------------------------------------------------------------------- (e)

#[test]
fn identity_version_and_endianness() {
    let base = baseline();
    let run = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut img = base.clone();
        f(&mut img);
        parse0(&img)
    };

    assert_eq!(
        run(&|i| forge_u32(i, 0, 8, 2)),
        Err(e(Section::Slot, 8, ParseKind::Version { found: 2 }))
    );
    assert_eq!(
        run(&|i| forge_u32(i, 0, 8, 0)),
        Err(e(Section::Slot, 8, ParseKind::Version { found: 0 }))
    );
    // Version stored big endian.
    assert_eq!(
        run(&|i| {
            i[8..12].copy_from_slice(&1u32.to_be_bytes());
            reseal_slot(i, 0);
        }),
        Err(e(
            Section::Slot,
            8,
            ParseKind::Version { found: 0x0100_0000 }
        ))
    );
    // Block size is part of the format identity.
    for bs in [1024u32, 0, 256, 512u32.swap_bytes()] {
        assert_eq!(
            run(&|i| forge_u32(i, 0, 12, bs)),
            Err(e(Section::Slot, 12, ParseKind::Identity)),
            "block size {bs}"
        );
    }
    // Every multi-byte field written big endian.
    assert_eq!(
        run(&|i| {
            let (g, c, mo, ec, eo) = (
                get_u64(i, 16),
                get_u64(i, 24),
                get_u64(i, 32),
                get_u32(i, 40),
                get_u64(i, 44),
            );
            i[8..12].copy_from_slice(&1u32.to_be_bytes());
            i[12..16].copy_from_slice(&512u32.to_be_bytes());
            i[16..24].copy_from_slice(&g.to_be_bytes());
            i[24..32].copy_from_slice(&c.to_be_bytes());
            i[32..40].copy_from_slice(&mo.to_be_bytes());
            i[40..44].copy_from_slice(&ec.to_be_bytes());
            i[44..52].copy_from_slice(&eo.to_be_bytes());
            reseal_slot(i, 0);
        }),
        Err(e(
            Section::Slot,
            8,
            ParseKind::Version { found: 0x0100_0000 }
        ))
    );

    // Magic: last byte, first byte, wrong data under a valid CRC, all zero
    // magic with other bytes set.
    for byte in [7usize, 0] {
        let mut found = *MAGIC_BYTES;
        found[byte] ^= 0xFF;
        assert_eq!(
            run(&|i| {
                i[byte] ^= 0xFF;
                reseal_slot(i, 0);
            }),
            Err(e(Section::Slot, 0, ParseKind::Magic { found })),
            "magic byte {byte}"
        );
    }
    assert_eq!(
        run(&|i| {
            i[0..8].fill(0);
            reseal_slot(i, 0);
        }),
        Err(e(Section::Slot, 0, ParseKind::Magic { found: [0; 8] }))
    );

    // CRC: a flipped field bit, and a flipped CRC bit, are corruption, not a
    // different version.
    let mut img = base.clone();
    img[20] ^= 0x01;
    let stored = get_u32(&img, 508);
    let computed = ref_crc(&img[..508]);
    assert_ne!(stored, computed);
    assert_eq!(
        parse0(&img),
        Err(e(Section::Slot, 508, ParseKind::Crc { stored, computed }))
    );
    let mut img = base.clone();
    img[11] ^= 0x01; // version byte, CRC not fixed
    assert!(matches!(
        parse0(&img),
        Err(ParseError {
            kind: ParseKind::Crc { .. },
            offset: 508,
            ..
        })
    ));

    // Reserved bytes.
    assert_eq!(
        run(&|i| {
            i[100] = 1;
            reseal_slot(i, 0);
        }),
        Err(e(Section::Slot, 100, ParseKind::Reserved))
    );
    assert_eq!(
        run(&|i| {
            i[507] = 0x80;
            reseal_slot(i, 0);
        }),
        Err(e(Section::Slot, 507, ParseKind::Reserved))
    );
}

#[test]
fn blank_slot_is_not_a_root() {
    let mut img = baseline();
    img[512..1024].fill(0);
    assert_eq!(
        parse_slot_at(&img[..], 512, 1, &CUR, Access::ReadOnly),
        Ok(Slot::Blank)
    );
    assert_eq!(
        parse_committed(&img[..], 1, &CUR, Access::ReadOnly),
        Err(e(Section::Slot, 512, ParseKind::Magic { found: [0; 8] }))
    );
}

#[test]
fn mandatory_features_gate_readonly_and_writable_open() {
    let base = baseline();
    let with = |mandatory: u64, optional: u64| {
        let mut img = base.clone();
        put_u64(&mut img, 52, mandatory);
        put_u64(&mut img, 60, optional);
        reseal_slot(&mut img, 0);
        img
    };
    let unsupported = |bit: u8| {
        e(
            Section::Slot,
            52,
            ParseKind::UnsupportedMandatoryFeature(bit),
        )
    };
    let ro = |img: &[u8], caps: &Capabilities| parse_committed(img, 0, caps, Access::ReadOnly);
    let rw = |img: &[u8], caps: &Capabilities| parse_committed(img, 0, caps, Access::Writable);

    // Unknown mandatory bit: neither mode opens it.
    let img = with(1 << 5, 0);
    assert_eq!(ro(&img, &CUR), Err(unsupported(5)));
    assert_eq!(rw(&img, &CUR), Err(unsupported(5)));

    // Understood for reading only: read-only opens, writable refuses.
    let read_only_caps = Capabilities {
        read_mandatory: 1 << 5,
        write_mandatory: 0,
    };
    let c = ro(&img, &read_only_caps).expect("read-only with understood bit");
    assert_eq!(c.root.features.mandatory, 1 << 5);
    assert_eq!(rw(&img, &read_only_caps), Err(unsupported(5)));

    // Understood for both.
    let both = Capabilities {
        read_mandatory: 1 << 5,
        write_mandatory: 1 << 5,
    };
    assert!(ro(&img, &both).is_ok() && rw(&img, &both).is_ok());

    // A write-only claim without read understanding does not count.
    let write_only = Capabilities {
        read_mandatory: 0,
        write_mandatory: 1 << 5,
    };
    assert_eq!(ro(&img, &write_only), Err(unsupported(5)));
    assert_eq!(rw(&img, &write_only), Err(unsupported(5)));

    // The lowest offending bit is named, known bits are skipped.
    let img = with((1 << 3) | (1 << 40), 0);
    assert_eq!(ro(&img, &CUR), Err(unsupported(3)));
    let know3 = Capabilities {
        read_mandatory: 1 << 3,
        write_mandatory: 1 << 3,
    };
    assert_eq!(ro(&img, &know3), Err(unsupported(40)));
    assert_eq!(ro(&with(1 << 63, 0), &CUR), Err(unsupported(63)));
    assert_eq!(ro(&with(1, 0), &CUR), Err(unsupported(0)));

    // Optional bits never block anything and are reported.
    let img = with(0, u64::MAX);
    let c = rw(&img, &CUR).expect("unknown optional bits are ignored");
    assert_eq!(
        c.root.features,
        Features {
            mandatory: 0,
            optional: u64::MAX
        }
    );
    assert_eq!(ro(&img, &CUR), Ok(c));

    // The feature check comes before limits: an unknown feature may change the
    // meaning of other fields.
    let mut img = with(1 << 9, 0);
    forge_u64(&mut img, 0, 24, u64::MAX);
    assert_eq!(ro(&img, &CUR), Err(unsupported(9)));
}

// ------------------------------------------------------------------- (f)

/// Records every read and can claim a different size than the bytes it holds.
struct Recording<'a> {
    bytes: &'a [u8],
    claimed: u64,
    reads: RefCell<Vec<(u64, usize)>>,
}

impl<'a> Recording<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            claimed: bytes.len() as u64,
            reads: RefCell::new(Vec::new()),
        }
    }
}

impl Source for Recording<'_> {
    fn size(&self) -> Result<u64, StorageError> {
        Ok(self.claimed)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), crate::cow_parse::ReadError> {
        self.reads.borrow_mut().push((offset, buf.len()));
        self.bytes.read_at(offset, buf)
    }
}

#[test]
fn limits_are_checked_before_any_allocation_or_map_read() {
    const { assert!(MAX_MAP_BYTES >= MAX_BLOCKS * 12) };
    let base = baseline();

    // Over the block limit: only the slot was read.
    let mut img = base.clone();
    forge_u64(&mut img, 0, 24, MAX_BLOCKS + 1);
    let src = Recording::new(&img);
    assert_eq!(
        parse_committed(&src, 0, &CUR, Access::ReadOnly),
        Err(e(
            Section::Slot,
            24,
            ParseKind::LimitExceeded(Limit::Blocks)
        ))
    );
    assert_eq!(*src.reads.borrow(), vec![(0, 512)]);

    // Absurd end_offset against a source that claims a huge size.
    let mut img = base.clone();
    forge_u64(&mut img, 0, 44, 1 << 40);
    let mut src = Recording::new(&img);
    src.claimed = 1 << 41;
    assert_eq!(
        parse_committed(&src, 0, &CUR, Access::ReadOnly),
        Err(e(
            Section::Slot,
            44,
            ParseKind::LimitExceeded(Limit::FileBytes)
        ))
    );
    assert_eq!(*src.reads.borrow(), vec![(0, 512)]);

    // A claimed huge size is not a reason to read more than the map: a valid
    // root over such a source reads the slot and exactly the 36 map bytes.
    let mut src = Recording::new(&base);
    src.claimed = 1 << 41;
    assert!(parse_committed(&src, 0, &CUR, Access::ReadOnly).is_ok());
    assert_eq!(*src.reads.borrow(), vec![(0, 512), (2560, 36)]);

    // block_count at the limit with a map that cannot fit is refused before
    // the (786432-byte) map buffer would be allocated or read.
    let mut img = base.clone();
    forge_u64(&mut img, 0, 24, MAX_BLOCKS);
    let src = Recording::new(&img);
    assert_eq!(
        parse_committed(&src, 0, &CUR, Access::ReadOnly),
        Err(e(Section::Slot, 32, ParseKind::OutOfRange))
    );
    assert_eq!(*src.reads.borrow(), vec![(0, 512)]);

    // `parse_map` does not trust a hand-built record either.
    let record = RootRecord {
        slot_offset: 0,
        generation: 2,
        block_count: u64::MAX,
        map_offset: 2560,
        map_crc: 0,
        end_offset: 3584,
        features: Features::default(),
    };
    let src = Recording::new(&base);
    assert_eq!(
        parse_map(&src, &record),
        Err(e(
            Section::Slot,
            24,
            ParseKind::LimitExceeded(Limit::Blocks)
        ))
    );
    assert!(src.reads.borrow().is_empty());
}

#[test]
fn map_at_the_block_limit_parses_and_one_more_does_not() {
    // MAX_BLOCKS distinct descriptors: data bytes are zero and descriptor CRCs
    // are not checked by `parse_map`.
    let n = MAX_BLOCKS as usize;
    let img = build(n, |_| [0u8; 512], false);
    let got = parse0(&img).expect("map at limit");
    assert_eq!(got.blocks.len(), n);
    assert_eq!(
        got.blocks.last().map(|b| b.phys),
        Some(1024 + 512 * (n as u64 - 1))
    );

    // Entirely distinct and in range, one above the limit: refused by count.
    let mut over = img;
    forge_u64(&mut over, 0, 24, MAX_BLOCKS + 1);
    assert_eq!(
        parse0(&over),
        Err(e(
            Section::Slot,
            24,
            ParseKind::LimitExceeded(Limit::Blocks)
        ))
    );
}

#[test]
fn empty_generation_and_empty_commit_parse() {
    let img = baseline();
    // Generation 1: no blocks, map at the header end.
    let c = parse_committed(&img[..], 1, &CUR, Access::Writable).unwrap();
    assert_eq!(
        (c.root.map_offset, c.root.end_offset, c.blocks.len()),
        (1024, 1024, 0)
    );
    // A generation with zero blocks after a commit: map empty, root copy only.
    let img = build(0, |_| [0u8; 512], true);
    let c = parse0(&img).unwrap();
    assert_eq!(
        (c.root.map_offset, c.root.end_offset, c.blocks.len()),
        (1024, 1536, 0)
    );
}

// ------------------------------------------------------------------- (g)

fn kind_name(k: &ParseKind) -> &'static str {
    match k {
        ParseKind::Truncated { .. } => "Truncated",
        ParseKind::Magic { .. } => "Magic",
        ParseKind::Version { .. } => "Version",
        ParseKind::UnsupportedMandatoryFeature(_) => "UnsupportedMandatoryFeature",
        ParseKind::Crc { .. } => "Crc",
        ParseKind::Overflow => "Overflow",
        ParseKind::OutOfRange => "OutOfRange",
        ParseKind::Overlap { .. } => "Overlap",
        ParseKind::Duplicate { .. } => "Duplicate",
        ParseKind::Generation => "Generation",
        ParseKind::Identity => "Identity",
        ParseKind::LimitExceeded(_) => "LimitExceeded",
        ParseKind::Reserved => "Reserved",
        ParseKind::Io(_) => "Io",
    }
}

/// Re-derives everything an `Ok` result claims from the raw bytes, using only
/// the independent CRC and literal offsets.
fn check_ok_independently(
    img: &[u8],
    slot: usize,
    caps: &Capabilities,
    access: Access,
    c: &Committed,
) {
    let so = slot * 512;
    let s = &img[so..so + 512];
    assert_eq!(&s[0..8], MAGIC_BYTES);
    assert_eq!(ref_crc(&s[..508]), get_u32(s, 508), "slot crc");
    assert_eq!(get_u32(s, 8), 1);
    assert_eq!(get_u32(s, 12), 512);
    assert!(s[68..508].iter().all(|&b| b == 0));
    let (generation, count) = (get_u64(s, 16), get_u64(s, 24));
    assert!(generation != 0 && generation != u64::MAX && generation as usize & 1 == slot);
    assert!(count <= MAX_BLOCKS);
    let (map_off, map_crc, end) = (get_u64(s, 32), get_u32(s, 40), get_u64(s, 44));
    let mandatory = get_u64(s, 52);
    let allowed = match access {
        Access::ReadOnly => caps.read_mandatory,
        Access::Writable => caps.read_mandatory & caps.write_mandatory,
    };
    assert_eq!(mandatory & !allowed, 0);
    assert_eq!(
        c.root,
        RootRecord {
            slot_offset: so as u64,
            generation,
            block_count: count,
            map_offset: map_off,
            map_crc,
            end_offset: end,
            features: Features {
                mandatory,
                optional: get_u64(s, 60)
            },
        }
    );

    assert!(end <= img.len() as u64 && end % 512 == 0 && (1024..=MAX_FILE_BYTES).contains(&end));
    let payload_end = if end > 1024 { end - 512 } else { end };
    let map_end = map_off + 12 * count;
    assert!(map_off >= 1024 && map_off % 512 == 0 && map_end <= payload_end);
    let map = &img[map_off as usize..map_end as usize];
    assert_eq!(ref_crc(map), map_crc, "map crc");
    assert_eq!(c.blocks.len() as u64, count);
    let mut seen = std::collections::BTreeSet::new();
    for (i, d) in c.blocks.iter().enumerate() {
        assert_eq!(d.phys, get_u64(map, 12 * i));
        assert_eq!(d.crc, get_u32(map, 12 * i + 8));
        assert!(d.phys >= 1024 && d.phys % 512 == 0 && d.phys + 512 <= payload_end);
        assert!(d.phys + 512 <= map_off || d.phys >= map_end);
        assert!(seen.insert(d.phys), "duplicate physical block");
    }
}

const FIELDS: [usize; 10] = [8, 12, 16, 24, 32, 40, 44, 52, 60, 100];

#[test]
fn deterministic_mutation_fuzz_never_panics_and_ok_is_consistent() {
    const RUNS: u64 = 20_000;
    const BLOCKS: usize = 6;
    let base = build(BLOCKS, pat, true);
    let (map_off, _, _, end) = layout(BLOCKS as u64);
    let (map_off, end) = (map_off as usize, end as usize);
    assert_eq!(base.len(), end);
    let interesting = [
        0u64,
        1,
        2,
        3,
        511,
        512,
        513,
        1024,
        1536,
        map_off as u64,
        base.len() as u64,
        base.len() as u64 + 512,
        MAX_BLOCKS,
        MAX_BLOCKS + 1,
        MAX_FILE_BYTES,
        MAX_FILE_BYTES + 1,
        u64::MAX / 2,
        u64::MAX - 511,
        u64::MAX - 1,
        u64::MAX,
    ];
    let mut rng = SplitMix64(0x5309_C0FF_EE15_BEEF);
    let mut kinds: BTreeMap<&'static str, u64> = BTreeMap::new();
    let (mut ok, mut errs, mut parses) = (0u64, 0u64, 0u64);

    for _ in 0..RUNS {
        let mut img = base.clone();
        let mut truncate_to = None;
        match rng.below(9) {
            0..=2 => {
                for _ in 0..=rng.below(3) {
                    let at = rng.below(img.len() as u64) as usize;
                    img[at] ^= (rng.below(255) + 1) as u8;
                }
            }
            3 => {
                for _ in 0..=rng.below(2) {
                    let at = rng.below(img.len() as u64) as usize;
                    img[at] = rng.next_u64() as u8;
                }
            }
            4 | 5 => {
                // Interesting value into a slot field, usually with a fixed CRC.
                let slot = (rng.below(2) * 512) as usize;
                let field = FIELDS[rng.below(FIELDS.len() as u64) as usize];
                let v = if rng.below(4) == 0 {
                    rng.next_u64()
                } else {
                    interesting[rng.below(interesting.len() as u64) as usize]
                };
                match field {
                    8 | 12 | 40 => put_u32(&mut img, slot + field, v as u32),
                    _ => put_u64(&mut img, slot + field, v),
                }
                if rng.below(4) != 0 {
                    reseal_slot(&mut img, slot);
                }
            }
            6 | 7 => {
                // Map entry field, usually with fixed map and slot CRCs.
                let i = rng.below(BLOCKS as u64) as usize;
                let v = interesting[rng.below(interesting.len() as u64) as usize];
                if rng.below(2) == 0 {
                    put_u64(&mut img, map_off + 12 * i, v);
                } else {
                    put_u32(&mut img, map_off + 12 * i + 8, v as u32);
                }
                if rng.below(4) != 0 {
                    reseal_map(&mut img, 0);
                }
            }
            _ => truncate_to = Some(rng.below(img.len() as u64 + 1) as usize),
        }
        if let Some(len) = truncate_to {
            img.truncate(len);
        }

        for slot in 0..2usize {
            for (caps, access) in [(&CUR, Access::ReadOnly), (&FULL, Access::Writable)] {
                parses += 1;
                match parse_committed(&img[..], slot, caps, access) {
                    Ok(c) => {
                        ok += 1;
                        check_ok_independently(&img, slot, caps, access, &c);
                    }
                    Err(err) => {
                        errs += 1;
                        *kinds.entry(kind_name(&err.kind)).or_default() += 1;
                        assert!(
                            err.offset <= (img.len() as u64).max(1024),
                            "error offset {} outside source of {}: {err}",
                            err.offset,
                            img.len()
                        );
                    }
                }
            }
        }
    }
    println!("fuzz: {RUNS} mutations, {parses} parses, ok={ok}, err={errs}, panics=0");
    for (k, n) in &kinds {
        println!("fuzz:   {k}: {n}");
    }
    assert_eq!(ok + errs, parses);
    assert!(ok > 0, "mutations never produce an accepted image");
    assert!(errs > 0);
    assert!(
        kinds.len() >= 9,
        "fuzz reached only {} error kinds: {kinds:?}",
        kinds.len()
    );
}

// ------------------------------------------------------------------- (h)
//
// Cycle and depth limits: not applicable. The prototype layout has no
// references between metadata objects (root -> one map -> data blocks), so no
// test is written; see "Not applicable" in the module documentation.
