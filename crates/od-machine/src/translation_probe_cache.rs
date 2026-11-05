//! Opt-in one-entry cache for the immutable, pure Long64 4 KiB probe.
//!
//! This is an experiment, not a production TLB. Borrowing `PhysicalMemory`
//! immutably for the cache lifetime prevents page-table mutation while an
//! entry is resident. Guest invalidation and tracked A/D effects are outside
//! this type's contract.

use crate::page_walk::{
    EffectivePermissions, GuestLinearAddress, Long64ProbeConfig, ProbeOutcome, ProbeReport,
    Translation4K, probe_long64_4k,
};
use crate::{GuestPhysicalAddress, PhysicalMemory};
use od_isa::memory::{AccessContext, AccessKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CacheTag {
    linear_page: u64,
    paging: Long64ProbeConfig,
    access: AccessKind,
    context: AccessContext,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CachedTranslation {
    page_base: GuestPhysicalAddress,
    permissions: EffectivePermissions,
    leaf_entry_address: GuestPhysicalAddress,
    leaf_entry_value: u64,
}

impl CachedTranslation {
    fn from_translation(translation: Translation4K) -> Self {
        Self {
            page_base: translation.page_base,
            permissions: translation.permissions,
            leaf_entry_address: translation.leaf_entry_address,
            leaf_entry_value: translation.leaf_entry_value,
        }
    }

    fn for_linear(self, linear: GuestLinearAddress) -> Translation4K {
        let offset = (linear.get() & 0xfff) as u16;
        Translation4K {
            linear,
            physical: GuestPhysicalAddress::new(self.page_base.get() | u64::from(offset)),
            page_base: self.page_base,
            offset,
            permissions: self.permissions,
            leaf_entry_address: self.leaf_entry_address,
            leaf_entry_value: self.leaf_entry_value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranslationProbeCacheOutcome {
    /// Returned metadata is reconstructed from the cached physical page.
    /// No page-walk trace was performed for this request.
    Hit(Translation4K),
    /// The existing pure walker ran. Its successful translations may have
    /// filled the single entry; faults and unsupported results never do.
    Walk(ProbeReport),
}

/// Experimental capacity-one cache bound to one immutable physical-memory
/// snapshot. The complete tag is compared on every lookup.
pub struct TranslationProbeCache<'a> {
    memory: &'a PhysicalMemory,
    entry: Option<(CacheTag, CachedTranslation)>,
}

impl<'a> TranslationProbeCache<'a> {
    pub fn new(memory: &'a PhysicalMemory) -> Self {
        Self {
            memory,
            entry: None,
        }
    }

    pub fn clear(&mut self) {
        self.entry = None;
    }

    pub fn is_empty(&self) -> bool {
        self.entry.is_none()
    }

    pub fn probe(
        &mut self,
        paging: &Long64ProbeConfig,
        linear: GuestLinearAddress,
        access: AccessKind,
        context: AccessContext,
    ) -> TranslationProbeCacheOutcome {
        let tag = CacheTag {
            linear_page: linear.get() >> 12,
            paging: *paging,
            access,
            context,
        };
        if let Some((cached_tag, translation)) = self.entry {
            if cached_tag == tag {
                return TranslationProbeCacheOutcome::Hit(translation.for_linear(linear));
            }
        }

        // Capacity one uses replacement on every miss. Clearing before the
        // walk also ensures an unsuccessful lookup is never retained.
        self.entry = None;
        let report = probe_long64_4k(self.memory, paging, linear, access, context);
        if let Ok(ProbeOutcome::Translated(translation)) = report.outcome {
            self.entry = Some((tag, CachedTranslation::from_translation(translation)));
        }
        TranslationProbeCacheOutcome::Walk(report)
    }
}

#[cfg(test)]
mod tests {
    use super::{TranslationProbeCache, TranslationProbeCacheOutcome};
    use crate::{
        GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, RamLimits,
    };
    use od_isa::memory::{
        AccessContext, AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel,
    };

    const ROOT: u64 = 0x1000;
    const PDPT: u64 = 0x2000;
    const PD: u64 = 0x3000;
    const PT: u64 = 0x4000;
    const PHYSICAL_PAGE: u64 = 0x9000;
    const LINEAR_PAGE: u64 = 0x400000;

    fn config(cr0: u64, cr3: u64, cr4: u64, efer: u64) -> super::Long64ProbeConfig {
        super::Long64ProbeConfig::new(cr0, cr3, cr4, efer).unwrap()
    }

    fn memory(pte: u64) -> PhysicalMemory {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x20_000).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x20_000)).unwrap();
        for (address, value) in [
            (ROOT, PDPT | 7),
            (PDPT, PD | 7),
            (PD + 16, PT | 7),
            (PT, pte),
        ] {
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        memory
    }

    fn explicit_ring3() -> AccessContext {
        AccessContext::default()
    }

    fn translated(outcome: TranslationProbeCacheOutcome) -> (bool, crate::Translation4K) {
        match outcome {
            TranslationProbeCacheOutcome::Hit(value) => (true, value),
            TranslationProbeCacheOutcome::Walk(report) => match report.outcome.unwrap() {
                crate::ProbeOutcome::Translated(value) => (false, value),
                other => panic!("expected translation, got {other:?}"),
            },
        }
    }

    #[test]
    fn successful_walk_then_hit_reconstructs_exact_translation_metadata() {
        let memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let mut cache = TranslationProbeCache::new(&memory);
        let first = GuestLinearAddress::new(LINEAR_PAGE + 0x13);
        let second = GuestLinearAddress::new(LINEAR_PAGE + 0x2a5);

        let (first_hit, first_translation) =
            translated(cache.probe(&paging, first, AccessKind::Read, explicit_ring3()));
        assert!(!first_hit);
        assert_eq!(first_translation.linear.get(), 0x0040_0013);
        assert_eq!(first_translation.physical.get(), 0x0000_9013);
        assert_eq!(first_translation.page_base.get(), PHYSICAL_PAGE);
        assert_eq!(first_translation.offset, 0x13);
        assert_eq!(first_translation.leaf_entry_address.get(), PT);
        assert_eq!(first_translation.leaf_entry_value, PHYSICAL_PAGE | 7);
        assert_eq!(first_translation.permissions.user, true);
        assert_eq!(first_translation.permissions.writable, true);
        assert_eq!(first_translation.permissions.execute_disabled, false);

        let (second_hit, second_translation) =
            translated(cache.probe(&paging, second, AccessKind::Read, explicit_ring3()));
        assert!(second_hit);
        assert_eq!(second_translation.linear.get(), 0x0040_02a5);
        assert_eq!(second_translation.physical.get(), 0x0000_92a5);
        assert_eq!(second_translation.offset, 0x2a5);
        assert_eq!(second_translation.leaf_entry_address.get(), PT);
        assert_eq!(second_translation.leaf_entry_value, PHYSICAL_PAGE | 7);
    }

    #[test]
    fn a_different_linear_page_replaces_the_single_entry() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        memory
            .write_u64(GuestPhysicalAddress::new(PT + 8), 0xa000 | 7, |_| {})
            .unwrap();
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let mut cache = TranslationProbeCache::new(&memory);
        let addresses = [
            (GuestLinearAddress::new(LINEAR_PAGE + 4), 0x9004, false),
            (GuestLinearAddress::new(LINEAR_PAGE + 8), 0x9008, true),
            (GuestLinearAddress::new(LINEAR_PAGE + 0x1004), 0xa004, false),
            (GuestLinearAddress::new(LINEAR_PAGE + 0x1008), 0xa008, true),
        ];
        for (linear, expected_physical, expected_hit) in addresses {
            let (hit, translation) =
                translated(cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()));
            assert_eq!(hit, expected_hit);
            assert_eq!(translation.physical.get(), expected_physical);
        }
    }

    #[test]
    fn every_tag_dimension_causes_a_walk_before_a_new_hit() {
        let memory = memory(PHYSICAL_PAGE | 7);
        let base = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let changed_cr0 = config(0x8000_0011, ROOT, 0x20, 0xd00);
        let other_root = config(0x8001_0011, ROOT | 1, 0x20, 0xd00);
        let changed_cr4 = config(0x8001_0011, ROOT, 0x220, 0xd00);
        let changed_efer = config(0x8001_0011, ROOT, 0x20, 0xd01);
        let context = explicit_ring3();
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 4);
        let mut cache = TranslationProbeCache::new(&memory);

        let ring0 = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let cases = [
            (base, AccessKind::Read, context, false),
            (base, AccessKind::Read, context, true),
            (changed_cr0, AccessKind::Read, context, false),
            (changed_cr0, AccessKind::Read, context, true),
            (other_root, AccessKind::Read, context, false),
            (other_root, AccessKind::Read, context, true),
            (changed_cr4, AccessKind::Read, context, false),
            (changed_cr4, AccessKind::Read, context, true),
            (changed_efer, AccessKind::Read, context, false),
            (changed_efer, AccessKind::Read, context, true),
            (base, AccessKind::Fetch, context, false),
            (base, AccessKind::Fetch, context, true),
            (base, AccessKind::Write, context, false),
            (base, AccessKind::Write, context, true),
            (base, AccessKind::Read, ring0, false),
            (base, AccessKind::Read, ring0, true),
            (base, AccessKind::Read, context.descriptor_table(), false),
            (base, AccessKind::Read, context.descriptor_table(), true),
        ];
        for (paging, access, context, expected_hit) in cases {
            let (hit, translation) = translated(cache.probe(&paging, linear, access, context));
            assert_eq!(hit, expected_hit);
            assert_eq!(translation.physical.get(), 0x9004);
        }
    }

    #[test]
    fn page_fault_is_walked_repeatedly_and_never_cached() {
        let memory = memory(0);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 9);
        let mut cache = TranslationProbeCache::new(&memory);
        for _ in 0..2 {
            match cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()) {
                TranslationProbeCacheOutcome::Walk(report) => {
                    assert!(matches!(
                        report.outcome,
                        Ok(crate::ProbeOutcome::PageFault(_))
                    ));
                    assert_eq!(report.trace.len(), 4);
                }
                TranslationProbeCacheOutcome::Hit(value) => {
                    panic!("fault was cached as {value:?}")
                }
            }
            assert!(cache.is_empty());
        }
    }

    #[test]
    fn clear_and_drop_remove_the_entry_and_allow_memory_replacement_after_drop() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x33);
        {
            let mut cache = TranslationProbeCache::new(&memory);
            assert!(matches!(
                cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
                TranslationProbeCacheOutcome::Walk(_)
            ));
            assert!(matches!(
                cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
                TranslationProbeCacheOutcome::Hit(_)
            ));
            cache.clear();
            assert!(cache.is_empty());
            assert!(matches!(
                cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
                TranslationProbeCacheOutcome::Walk(_)
            ));
        }
        memory
            .write_u64(GuestPhysicalAddress::new(PT), 0xa000 | 7, |_| {})
            .unwrap();
        let mut replacement = TranslationProbeCache::new(&memory);
        let (_, translation) =
            translated(replacement.probe(&paging, linear, AccessKind::Read, explicit_ring3()));
        assert_eq!(translation.physical.get(), 0xa033);
    }

    #[test]
    fn unsupported_outcome_is_not_cached() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        memory
            .write_u64(GuestPhysicalAddress::new(PD + 16), PT | 0x87, |_| {})
            .unwrap();
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE);
        let mut cache = TranslationProbeCache::new(&memory);
        for _ in 0..2 {
            match cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()) {
                TranslationProbeCacheOutcome::Walk(report) => {
                    assert_eq!(report.trace.len(), 3);
                    assert!(matches!(
                        report.outcome,
                        Err(crate::ProbeError::UnsupportedPageSize {
                            level: crate::PageTableLevel::Pd,
                            entry_address,
                            entry_value: 0x4087,
                        }) if entry_address.get() == PD + 16
                    ));
                }
                TranslationProbeCacheOutcome::Hit(value) => {
                    panic!("unsupported result was cached as {value:?}")
                }
            }
            assert!(cache.is_empty());
        }
    }
}
