//! PERF-10: the decoded-instruction cache of the interpreter.
//!
//! `od_isa::long_decode::decode` reads an instruction one byte at a time (one `fetch` per byte,
//! several passes for the SSE/MMX forms). This cache remembers its result, so the step of a hot
//! instruction does not read or decode its bytes again. It lives in `od-machine`, next to
//! [`crate::fetch_translation_cache::FetchTranslationCache`], because its hit test uses that cache
//! and the page generations of [`crate::PhysicalMemory`]. It is not an artifact of the JIT: it
//! creates nothing, it only remembers what `decode` returned. `od-isa` knows two methods of the
//! `GuestMemory` trait with a "no cache" default (`cached_instruction`, `remember_instruction`);
//! the adapter of the set loop (`paged_memory_adapter.rs`) answers them.
//!
//! An entry is keyed by the linear RIP, the physical place of the first byte (RAM region and byte
//! offset inside it), the generation of the 4 KiB code page that holds it (read BEFORE the bytes,
//! the reader's rule of `PhysicalMemory::code_page_generation`) and the `AccessContext`. A hit needs
//! all of them, and the adapter adds the gate of its own (see `PagedMovMemoryAdapter::cached_instruction`):
//! the fetch translation cache still holds the translation of the RIP's page for the current paging
//! snapshot and context (so fetch rights, U/S, SMEP and the access bits are those of the byte-per-byte
//! fetch), and the page generation is the stored one. Every committed RAM write raises the
//! generation of its page, so self-modifying code, a device write and a DMA between quanta can only
//! cause a miss, never a stale hit. An entry is made only from an instruction whose every fetched
//! byte lies on the page of the RIP.
//!
//! The cache is owned by the board and lives between quanta. It is bound to one `PhysicalMemory`
//! (a different memory, a board reset and a snapshot restore empty it: generations of another memory
//! start again from 1). The table is a direct-mapped array allocated on the first insert; its size
//! is a constructor parameter (a power of two).

use crate::physical_memory::RamCodePage;
use od_isa::long_decode::Instruction;
use od_isa::memory::AccessContext;
use std::cell::{Cell, RefCell};

/// The default size of the table: 2^16 entries (PERF-10 step 9 measures 2^14, 2^16 and 2^18).
pub(crate) const DEFAULT_DECODED_INSTRUCTION_SLOTS: usize = 1 << 16;

#[derive(Clone, Copy)]
struct Entry {
    rip: u64,
    region: usize,
    offset: usize,
    generation: u64,
    context: AccessContext,
    instruction: Instruction,
}

/// What the cache counted (the tests of PERF-10 and the measurement read it).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DecodedInstructionCacheStats {
    /// Lookups that returned an instruction.
    pub(crate) hits: u64,
    /// Lookups the gate allowed that found no valid entry (a different key or an older generation).
    pub(crate) misses: u64,
    /// Entries stored.
    pub(crate) inserts: u64,
    /// Times the whole table was emptied (`reset`, or another memory bound).
    pub(crate) resets: u64,
}

pub(crate) struct DecodedInstructionCache {
    shift: u32,
    table: RefCell<Vec<Option<Entry>>>,
    live: Cell<usize>,
    memory: Cell<usize>,
    stats: Cell<DecodedInstructionCacheStats>,
}

impl Default for DecodedInstructionCache {
    fn default() -> Self {
        Self::new(DEFAULT_DECODED_INSTRUCTION_SLOTS)
    }
}

impl DecodedInstructionCache {
    /// A cache of `slots` entries, rounded up to a power of two (at least 2). Nothing is allocated
    /// until the first entry is stored.
    pub(crate) fn new(slots: usize) -> Self {
        let slots = slots.max(2).next_power_of_two();
        Self {
            shift: 64 - slots.trailing_zeros(),
            table: RefCell::new(Vec::new()),
            live: Cell::new(0),
            memory: Cell::new(0),
            stats: Cell::new(DecodedInstructionCacheStats::default()),
        }
    }

    pub(crate) fn slots(&self) -> usize {
        1usize << (64 - self.shift)
    }

    /// The number of occupied slots, counted over the table itself (not from `live`): the tests
    /// that a reset empties the table read this.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> usize {
        self.table.borrow().iter().flatten().count()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stats(&self) -> DecodedInstructionCacheStats {
        self.stats.get()
    }

    fn count(&self, update: impl FnOnce(&mut DecodedInstructionCacheStats)) {
        let mut stats = self.stats.get();
        update(&mut stats);
        self.stats.set(stats);
    }

    /// Empties the table (the board reset and the snapshot restore call this). The counters stay.
    pub(crate) fn reset(&self) {
        if self.live.get() != 0 {
            self.table
                .borrow_mut()
                .iter_mut()
                .for_each(|slot| *slot = None);
            self.live.set(0);
        }
        self.memory.set(0);
        self.count(|stats| stats.resets += 1);
    }

    /// Ties the cache to the memory of the run that is about to start (`identity` is the address
    /// of its `PhysicalMemory`): another memory empties the table. A cache that has stored nothing
    /// only remembers the identity.
    pub(crate) fn bind_memory(&self, identity: usize) {
        let bound = self.memory.get();
        if bound == identity {
            return;
        }
        if bound != 0 {
            self.reset();
        }
        self.memory.set(identity);
    }

    fn index(&self, region: usize, offset: usize) -> usize {
        let key = (offset as u64) ^ ((region as u64) << 48);
        (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> self.shift) as usize
    }

    /// The remembered instruction for exactly this key, or `None`. `page` carries the generation
    /// the caller read before it would read the bytes.
    pub(crate) fn lookup(
        &self,
        rip: u64,
        context: AccessContext,
        page: RamCodePage,
    ) -> Option<Instruction> {
        let hit = {
            let table = self.table.borrow();
            // Compare the key through a reference; only a matching entry gives up its instruction.
            table
                .get(self.index(page.region, page.offset))
                .and_then(Option::as_ref)
                .filter(|entry| {
                    entry.rip == rip
                        && entry.region == page.region
                        && entry.offset == page.offset
                        && entry.generation == page.generation
                        && entry.context == context
                })
                .map(|entry| entry.instruction)
        };
        self.count(|stats| match hit {
            Some(_) => stats.hits += 1,
            None => stats.misses += 1,
        });
        hit
    }

    /// Stores the result of a decode made from the bytes of `page` (its generation read before
    /// them), replacing whatever held the slot.
    pub(crate) fn insert(
        &self,
        rip: u64,
        context: AccessContext,
        page: RamCodePage,
        instruction: Instruction,
    ) {
        let mut table = self.table.borrow_mut();
        if table.is_empty() {
            table.resize(self.slots(), None);
        }
        let slot = self.index(page.region, page.offset);
        if table[slot].is_none() {
            self.live.set(self.live.get() + 1);
        }
        table[slot] = Some(Entry {
            rip,
            region: page.region,
            offset: page.offset,
            generation: page.generation,
            context,
            instruction,
        });
        drop(table);
        self.count(|stats| stats.inserts += 1);
    }
}

/// What the tests of this cache, and the tests of the board that owns one, share.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use od_isa::long_decode::{DecodeError, decode};
    use od_isa::memory::{AccessOrigin, GuestMemory, InstructionMode, MemoryError, PrivilegeLevel};

    pub(crate) fn context(privilege: PrivilegeLevel) -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        }
    }

    struct Code(Vec<u8>);

    impl GuestMemory for Code {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.0
                .get(address as usize)
                .copied()
                .ok_or_else(|| MemoryError::Backend("out of code".into()))
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("no data".into()))
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("no data".into()))
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    pub(crate) fn decoded(bytes: &[u8]) -> Result<Instruction, DecodeError> {
        decode(&mut Code(bytes.to_vec()), 0, context(PrivilegeLevel::Ring0))
    }

    pub(crate) fn page(offset: usize, generation: u64) -> RamCodePage {
        RamCodePage {
            region: 0,
            offset,
            generation,
        }
    }

    /// Stores a `NOP` under a key of its own and returns the key: `lookup` with it answers
    /// `Some` until the table is emptied.
    pub(crate) fn insert_probe(
        cache: &DecodedInstructionCache,
    ) -> (u64, AccessContext, RamCodePage, Instruction) {
        let (rip, ring0, probe) = (0x7000, context(PrivilegeLevel::Ring0), page(0x70, 1));
        let nop = decoded(&[0x90]).unwrap();
        cache.insert(rip, ring0, probe, nop);
        assert_eq!(
            cache.lookup(rip, ring0, probe),
            Some(nop),
            "the probe is in"
        );
        (rip, ring0, probe, nop)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{context, decoded, page};
    use super::*;
    use od_isa::memory::PrivilegeLevel;

    #[test]
    fn a_hit_needs_rip_place_generation_and_context() {
        let cache = DecodedInstructionCache::new(16);
        let ring0 = context(PrivilegeLevel::Ring0);
        let nop = decoded(&[0x90]).unwrap();
        assert_eq!(cache.lookup(0x1000, ring0, page(0x40, 1)), None);
        cache.insert(0x1000, ring0, page(0x40, 1), nop);
        assert_eq!(cache.lookup(0x1000, ring0, page(0x40, 1)), Some(nop));
        // Every part of the key is compared.
        assert_eq!(
            cache.lookup(0x2000, ring0, page(0x40, 1)),
            None,
            "linear RIP"
        );
        assert_eq!(cache.lookup(0x1000, ring0, page(0x41, 1)), None, "offset");
        assert_eq!(
            cache.lookup(0x1000, ring0, page(0x40, 2)),
            None,
            "generation"
        );
        assert_eq!(
            cache.lookup(0x1000, context(PrivilegeLevel::Ring3), page(0x40, 1)),
            None,
            "context"
        );
        let other_region = RamCodePage {
            region: 1,
            ..page(0x40, 1)
        };
        assert_eq!(cache.lookup(0x1000, ring0, other_region), None, "region");
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.inserts), (1, 1));
    }

    /// Keys that fall into one slot must still be told apart by region, offset and RIP: the direct
    /// mapping alone is no proof of identity.
    #[test]
    fn keys_of_one_slot_are_told_apart_by_every_part() {
        let cache = DecodedInstructionCache::new(2);
        let ring0 = context(PrivilegeLevel::Ring0);
        let nop = decoded(&[0x90]).unwrap();
        let slot = cache.index(0, 0x40);
        let same_slot_offset = (0x41..0x1000)
            .find(|offset| cache.index(0, *offset) == slot)
            .expect("two slots: half of the offsets collide");
        let same_slot_region = (1..64)
            .find(|region| cache.index(*region, 0x40) == slot)
            .expect("two slots: half of the regions collide");
        cache.insert(0x1000, ring0, page(0x40, 1), nop);
        assert_eq!(cache.lookup(0x1000, ring0, page(0x40, 1)), Some(nop));
        assert_eq!(
            cache.lookup(0x1000, ring0, page(same_slot_offset, 1)),
            None,
            "offset"
        );
        let other_region = RamCodePage {
            region: same_slot_region,
            ..page(0x40, 1)
        };
        assert_eq!(cache.lookup(0x1000, ring0, other_region), None, "region");
        assert_eq!(cache.lookup(0x1001, ring0, page(0x40, 1)), None, "RIP");
    }

    #[test]
    fn a_collision_replaces_the_entry_and_the_size_is_rounded_to_a_power_of_two() {
        let cache = DecodedInstructionCache::new(2);
        assert_eq!(DecodedInstructionCache::new(3).slots(), 4);
        assert_eq!(DecodedInstructionCache::new(0).slots(), 2);
        assert_eq!(cache.slots(), 2);
        let ring0 = context(PrivilegeLevel::Ring0);
        let nop = decoded(&[0x90]).unwrap();
        for offset in 0..64 {
            cache.insert(0x1000 + offset as u64, ring0, page(offset, 1), nop);
        }
        let live = (0..64)
            .filter(|offset| {
                cache
                    .lookup(0x1000 + *offset as u64, ring0, page(*offset, 1))
                    .is_some()
            })
            .count();
        assert!((1..=2).contains(&live), "{live}");
    }

    #[test]
    fn reset_and_another_memory_empty_the_table_and_nothing_is_allocated_before_an_insert() {
        let cache = DecodedInstructionCache::default();
        assert_eq!(cache.slots(), DEFAULT_DECODED_INSTRUCTION_SLOTS);
        assert!(cache.table.borrow().is_empty());
        let ring0 = context(PrivilegeLevel::Ring0);
        let nop = decoded(&[0x90]).unwrap();
        cache.bind_memory(0x1000);
        cache.insert(0x10, ring0, page(0x10, 1), nop);
        cache.bind_memory(0x1000);
        assert_eq!(cache.lookup(0x10, ring0, page(0x10, 1)), Some(nop));
        assert_eq!(cache.entries(), 1);
        cache.bind_memory(0x2000);
        assert_eq!(cache.entries(), 0, "another memory empties the table");
        assert_eq!(cache.lookup(0x10, ring0, page(0x10, 1)), None);
        cache.insert(0x10, ring0, page(0x10, 1), nop);
        assert_eq!(cache.entries(), 1);
        cache.reset();
        assert_eq!(cache.entries(), 0, "reset empties the table");
        assert_eq!(cache.lookup(0x10, ring0, page(0x10, 1)), None);
        assert_eq!(cache.stats().resets, 2);
        // The table is usable again after the reset.
        cache.insert(0x10, ring0, page(0x10, 1), nop);
        assert_eq!(cache.lookup(0x10, ring0, page(0x10, 1)), Some(nop));
    }
}
