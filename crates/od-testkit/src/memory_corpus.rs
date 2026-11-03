//! Deterministic inputs for the bounded two-site checked-memory corpus.
//!
//! `S3_34A_SEQUENCES` preserves the original two S3-34a forms. The later
//! load-then-load extension remains available to the host correctness corpus,
//! but is deliberately not part of the S3-34a benchmark subset.

use serde::{Deserialize, Serialize};

pub const S3_34A_SEEDS: [u64; 3] = [
    0x5343_3441_0000_0001,
    0x5343_3441_0000_0002,
    0x5343_3441_0000_0003,
];
pub const S3_34A_CASES_PER_SEED: usize = 400;
pub const S3_34A_SEQUENCES: [MemoryCorpusSequence; 2] = [
    MemoryCorpusSequence::StoreThenLoad,
    MemoryCorpusSequence::LoadThenStore,
];
pub const MEMORY_CORPUS_SEQUENCES: [MemoryCorpusSequence; 3] = [
    MemoryCorpusSequence::StoreThenLoad,
    MemoryCorpusSequence::LoadThenStore,
    MemoryCorpusSequence::LoadThenLoad,
];
pub const MEMORY_CORPUS_RAM_LEN: usize = 128;

/// Fixed S3-26f corpus dimensions for the exact qword Store→Store profile.
pub const S3_26F_SEEDS: [u64; 3] = [
    0x5333_3236_4600_0001,
    0x5333_3236_4600_0002,
    0x5333_3236_4600_0003,
];
pub const S3_26F_CASES_PER_SEED: usize = 400;
pub const S3_26F_SEQUENCE: MemoryCorpusSequence = MemoryCorpusSequence::StoreThenStore;

/// Fixed S3-34dm corpus dimensions for the Address32 RSP-base Dword store.
pub const S3_34DM_SEEDS: [u64; 3] = [
    0x5333_3444_4d00_0001,
    0x5333_3444_4d00_0002,
    0x5333_3444_4d00_0003,
];
pub const S3_34DM_CASES_PER_SEED: usize = 400;
pub const S3_34DM_RAM_LEN: usize = 12;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCorpusSequence {
    StoreThenLoad,
    LoadThenStore,
    LoadThenLoad,
    StoreThenStore,
}

impl MemoryCorpusSequence {
    pub const fn bytes(self) -> [u8; 6] {
        match self {
            Self::StoreThenLoad => [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b],
            Self::LoadThenStore => [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02],
            Self::LoadThenLoad => [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b],
            Self::StoreThenStore => [0x48, 0x89, 0x03, 0x48, 0x89, 0x02],
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MemoryCorpusInput {
    pub sequence: MemoryCorpusSequence,
    pub gpr: [u64; 16],
    pub flags: u64,
    pub first_address: u64,
    pub second_address: u64,
    pub ram: [u8; MEMORY_CORPUS_RAM_LEN],
}

/// One bounded input for the exact `67 89 04 24` dword store scenario.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DwordStoreCorpusInput {
    pub gpr: [u64; 16],
    pub flags: u64,
    /// Low 32-bit ESP value used as the zero-extended effective address.
    pub address: u32,
    /// Bytes for offsets -4 through +7 from `address`; negative offsets that
    /// underflow address zero are omitted when the test memory is built.
    pub ram: [u8; S3_34DM_RAM_LEN],
}

#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Generate one input using the memory corpus's fixed seed/index algorithm.
///
/// When `forced_sequence` is `None`, the same seed/index bit selects one of
/// the original S3-34a sequences. Callers selecting a frozen profile should
/// force its sequence and iterate only that profile's seed/index dimensions.
pub fn generate_memory_corpus_input(
    seed: u64,
    index: usize,
    forced_sequence: Option<MemoryCorpusSequence>,
) -> MemoryCorpusInput {
    let mut rng = Rng(seed ^ (index as u64).wrapping_mul(0xd6e8_feb8_6659_fd93));
    let generated_sequence = if rng.next() & 1 == 0 {
        MemoryCorpusSequence::StoreThenLoad
    } else {
        MemoryCorpusSequence::LoadThenStore
    };
    let sequence = forced_sequence.unwrap_or(generated_sequence);
    let mut ram = [0; MEMORY_CORPUS_RAM_LEN];
    for chunk in ram.chunks_mut(8) {
        let bytes = rng.next().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }

    let mut first_address = 16 + (rng.next() % 88);
    if sequence == MemoryCorpusSequence::LoadThenLoad {
        // Exercise the full fixture boundary plus unaligned qword addresses.
        first_address = [0, 1, 7, 111, 112, 120][index % 6];
    }
    let mode = index % 3;
    let second_address = match mode {
        0 => first_address,
        1 => {
            let candidate = 16 + (rng.next() % 87);
            if candidate >= first_address {
                candidate + 1
            } else {
                candidate
            }
        }
        _ => {
            let distance = (rng.next() % 7 + 1) as i64;
            let delta = if first_address <= 22 {
                distance
            } else if first_address >= 97 || rng.next() & 1 == 0 {
                -distance
            } else {
                distance
            };
            (first_address as i64 + delta) as u64
        }
    };
    let mut gpr = [0; 16];
    for value in &mut gpr {
        *value = rng.next();
    }
    // All sequences use RBX for the first address. LoadThenStore and
    // StoreThenStore use RDX for their second address; LoadThenLoad reads the
    // same RBX range twice.
    gpr[3] = first_address;
    gpr[2] = if matches!(
        sequence,
        MemoryCorpusSequence::LoadThenStore | MemoryCorpusSequence::StoreThenStore
    ) {
        second_address
    } else {
        rng.next()
    };

    MemoryCorpusInput {
        sequence,
        gpr,
        flags: rng.next() & 0x8d5,
        first_address,
        second_address: if matches!(
            sequence,
            MemoryCorpusSequence::LoadThenStore | MemoryCorpusSequence::StoreThenStore
        ) {
            second_address
        } else {
            first_address
        },
        ram,
    }
}

/// Generate one deterministic S3-26f input using the established two-address
/// distribution and the profile's fixed seed/index algorithm.
pub fn generate_s3_26f_store_store_input(seed: u64, index: usize) -> MemoryCorpusInput {
    generate_memory_corpus_input(seed, index, Some(S3_26F_SEQUENCE))
}

/// Generate one deterministic S3-34dm input. Each seed/index pair selects a
/// stable address class and independently generated CPU/RAM bytes.
pub fn generate_s3_34dm_dword_store_input(seed: u64, index: usize) -> DwordStoreCorpusInput {
    let mut rng = Rng(seed ^ (index as u64).wrapping_mul(0xd6e8_feb8_6659_fd93));
    let address = match index % 4 {
        0 => 0,
        1 => 1 + 2 * (rng.next() as u32 % 0x800),
        2 => 0x1000 + (rng.next() as u32 % 0x7fff_ffff),
        // Every fourth input stores across the 4 GiB boundary.
        _ => [0xffff_fffd, 0xffff_fffe, 0xffff_ffff][index / 4 % 3],
    };
    let mut ram = [0; S3_34DM_RAM_LEN];
    for chunk in ram.chunks_mut(8) {
        let bytes = rng.next().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    let mut gpr = [0; 16];
    for value in &mut gpr {
        *value = rng.next();
    }
    // Address-size override discards the high half of RSP for the effective
    // address, while retaining randomized high bits as an explicit input.
    gpr[4] = (rng.next() & 0xffff_ffff_0000_0000) | u64::from(address);

    DwordStoreCorpusInput {
        gpr,
        flags: rng.next() & 0x8d5,
        address,
        ram,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        MEMORY_CORPUS_SEQUENCES, MemoryCorpusSequence, S3_26F_CASES_PER_SEED, S3_26F_SEEDS,
        S3_26F_SEQUENCE, S3_34A_CASES_PER_SEED, S3_34A_SEEDS, S3_34A_SEQUENCES,
        generate_memory_corpus_input, generate_s3_26f_store_store_input,
    };

    #[test]
    fn s3_34a_is_the_original_two_sequence_subset() {
        assert_eq!(
            S3_34A_SEQUENCES,
            [
                MemoryCorpusSequence::StoreThenLoad,
                MemoryCorpusSequence::LoadThenStore,
            ]
        );
        assert_eq!(MEMORY_CORPUS_SEQUENCES.len(), 3);
        assert_eq!(S3_34A_CASES_PER_SEED, 400);
    }

    #[test]
    fn s3_34a_inputs_are_deterministic_unique_and_match_the_frozen_digest() {
        let mut unique = HashSet::new();
        let mut digest_bytes = Vec::new();
        for sequence in S3_34A_SEQUENCES {
            for seed in S3_34A_SEEDS {
                for index in 0..S3_34A_CASES_PER_SEED {
                    let input = generate_memory_corpus_input(seed, index, Some(sequence));
                    assert_eq!(
                        input,
                        generate_memory_corpus_input(seed, index, Some(sequence))
                    );
                    assert!(unique.insert(input.clone()));
                    assert!(input.first_address + 8 <= input.ram.len() as u64);
                    assert!(input.second_address + 8 <= input.ram.len() as u64);

                    digest_bytes.extend_from_slice(&sequence.bytes());
                    digest_bytes.extend_from_slice(&seed.to_le_bytes());
                    digest_bytes.extend_from_slice(&(index as u64).to_le_bytes());
                    digest_bytes.extend_from_slice(&input.flags.to_le_bytes());
                    digest_bytes.extend_from_slice(&input.first_address.to_le_bytes());
                    digest_bytes.extend_from_slice(&input.second_address.to_le_bytes());
                    for value in input.gpr {
                        digest_bytes.extend_from_slice(&value.to_le_bytes());
                    }
                    digest_bytes.extend_from_slice(&input.ram);
                }
            }
        }
        assert_eq!(unique.len(), 2 * 3 * 400);
        assert_eq!(
            crate::sha256_hex(&digest_bytes),
            "b09478b556aa91234d8c453f20e70f637222c5de2d7369d305ffe7c78caae540"
        );
    }

    #[test]
    fn s3_26f_store_store_inputs_are_deterministic_unique_and_use_two_addresses() {
        assert_eq!(S3_26F_SEQUENCE.bytes(), [0x48, 0x89, 0x03, 0x48, 0x89, 0x02]);
        let mut unique = HashSet::new();
        for seed in S3_26F_SEEDS {
            for index in 0..S3_26F_CASES_PER_SEED {
                let input = generate_s3_26f_store_store_input(seed, index);
                assert_eq!(
                    input,
                    generate_s3_26f_store_store_input(seed, index)
                );
                assert!(unique.insert(input.clone()));
                assert_eq!(input.sequence, S3_26F_SEQUENCE);
                assert_eq!(
                    input.sequence.bytes(),
                    [0x48, 0x89, 0x03, 0x48, 0x89, 0x02]
                );
                assert_eq!(input.gpr[3], input.first_address);
                assert_eq!(input.gpr[2], input.second_address);
                assert!(input.first_address + 8 <= input.ram.len() as u64);
                assert!(input.second_address + 8 <= input.ram.len() as u64);
            }
        }
        assert_eq!(unique.len(), S3_26F_SEEDS.len() * S3_26F_CASES_PER_SEED);
    }

    #[test]
    fn unforced_generation_only_selects_original_sequences() {
        for index in 0..S3_34A_CASES_PER_SEED {
            let input = generate_memory_corpus_input(S3_34A_SEEDS[0], index, None);
            assert!(S3_34A_SEQUENCES.contains(&input.sequence));
        }
    }
}
