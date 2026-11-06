//! Opt-in one-entry cache for the immutable, pure Long64 4 KiB probe.
//!
//! This is an experiment, not a production TLB. Borrowing `PhysicalMemory`
//! immutably for the cache lifetime prevents page-table mutation while an
//! entry is resident. Dropping the cache ends that borrow, after which callers
//! may mutate page tables and create a new cache over the updated snapshot.
//! Guest invalidation and tracked A/D effects are outside this type's contract.

use crate::page_walk::{
    EffectivePermissions, GuestLinearAddress, Long64ProbeConfig, ProbeOutcome, ProbeReport,
    Translation4K, probe_long64_4k,
};
use crate::{GuestPhysicalAddress, PhysicalMemory};
use od_isa::TranslationInvalidation;
use od_isa::memory::{AccessContext, AccessKind};

/// Confirms that an experimental guest invalidation effect was consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranslationInvalidationConsumption {
    pub consumed: bool,
    pub removed_cached_entry: bool,
}

/// Consume a CPU effect before the caller performs another cache lookup.
/// The capacity-one experiment conservatively clears its full resident entry.
pub fn consume_translation_invalidation(
    cache: &mut TranslationProbeCache<'_>,
    effect: TranslationInvalidation,
) -> TranslationInvalidationConsumption {
    match effect {
        TranslationInvalidation::Invlpg { .. } => {
            let removed_cached_entry = !cache.is_empty();
            cache.clear();
            TranslationInvalidationConsumption {
                consumed: true,
                removed_cached_entry,
            }
        }
    }
}

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
    use super::{
        TranslationProbeCache, TranslationProbeCacheOutcome, consume_translation_invalidation,
    };
    use crate::{
        GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, RamLimits,
    };
    use od_isa::memory::{
        AccessContext, AccessKind, AccessOrigin, GuestMemory, InstructionMode, MemoryError,
        PrivilegeLevel,
    };
    use od_isa::{
        CpuActivity, InvlpgExit, InvlpgLong64V1, LongModeState, OperandSize, Register, StiShadow,
        TranslationInvalidation, step_invlpg,
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

    struct FetchOnlySnapshot<'a> {
        memory: &'a PhysicalMemory,
        paging: super::Long64ProbeConfig,
        fetches: Vec<u64>,
        fail_fetch_at: Option<u64>,
    }

    impl GuestMemory for FetchOnlySnapshot<'_> {
        fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
            self.fetches.push(address);
            if self.fail_fetch_at == Some(address) {
                return Err(MemoryError::Backend(
                    "injected instruction fetch failure".into(),
                ));
            }
            match crate::probe_long64_4k(
                self.memory,
                &self.paging,
                GuestLinearAddress::new(address),
                AccessKind::Fetch,
                context,
            )
            .outcome
            {
                Ok(crate::ProbeOutcome::Translated(translation)) => self
                    .memory
                    .read_u8(translation.physical)
                    .map_err(|error| MemoryError::Backend(format!("fetch snapshot read: {error}"))),
                other => Err(MemoryError::Backend(format!(
                    "fetch translation: {other:?}"
                ))),
            }
        }

        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            panic!("INVLPG must not read target or data memory")
        }

        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            panic!("INVLPG must not write memory")
        }

        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("INVLPG must not access data memory")
        }

        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("INVLPG must not access data memory")
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("INVLPG must not access data memory")
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            panic!("INVLPG must not fence data memory")
        }
    }

    #[test]
    fn invlpg_consumption_forces_walk_after_hit_without_mutating_snapshot() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let code = [0x0f, 0x01, 0x3b];
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &code,
                |_| {},
            )
            .unwrap();
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x24),
                &code,
                |_| {},
            )
            .unwrap();
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let ring0 = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        let table_snapshot = [ROOT, PDPT, PD + 16, PT]
            .map(|address| memory.read_u64(GuestPhysicalAddress::new(address)).unwrap());
        let mut cache = TranslationProbeCache::new(&memory);
        let first_walk = cache.probe(&paging, linear, AccessKind::Read, ring0);
        let first = match first_walk {
            TranslationProbeCacheOutcome::Walk(report) => match report.outcome.unwrap() {
                crate::ProbeOutcome::Translated(translation) => translation,
                other => panic!("expected successful first walk, got {other:?}"),
            },
            TranslationProbeCacheOutcome::Hit(_) => panic!("first lookup must walk"),
        };
        assert!(
            matches!(
                cache.probe(&paging, linear, AccessKind::Read, ring0),
                TranslationProbeCacheOutcome::Hit(_)
            ),
            "second lookup must hit the resident entry"
        );

        let mut cpu = LongModeState::default();
        cpu.rip = linear.get();
        // A different page proves the experiment clears the whole resident
        // entry rather than matching the requested page.
        cpu.gpr[Register::Rbx as usize] = 0x1234_5678;
        cpu.cr0 = Some(0x8001_0033);
        cpu.cr3 = Some(ROOT);
        cpu.cr4 = Some(0x20);
        cpu.activity = Some(CpuActivity::Running);
        cpu.sti_shadow = Some(StiShadow::Clear);
        let mut fetch_only = FetchOnlySnapshot {
            memory: &memory,
            paging,
            fetches: Vec::new(),
            fail_fetch_at: None,
        };
        let cpu_result = step_invlpg(
            InvlpgLong64V1,
            &mut cpu,
            &mut fetch_only,
            ring0,
            0xd00,
            od_isa::INTEGER_FLAGS,
        );
        assert_eq!(
            fetch_only.fetches,
            [linear.get(), linear.get() + 1, linear.get() + 2]
        );
        let InvlpgExit::TranslationInvalidation(effect @ TranslationInvalidation::Invlpg { .. }) =
            cpu_result.exit
        else {
            panic!(
                "expected CPU invalidation effect, got {:?}",
                cpu_result.exit
            )
        };
        let consumed = consume_translation_invalidation(&mut cache, effect);
        assert_eq!(
            consumed,
            super::TranslationInvalidationConsumption {
                consumed: true,
                removed_cached_entry: true
            }
        );

        let after_walk = cache.probe(&paging, linear, AccessKind::Read, ring0);
        let after = match after_walk {
            TranslationProbeCacheOutcome::Walk(report) => match report.outcome.unwrap() {
                crate::ProbeOutcome::Translated(translation) => translation,
                other => panic!("expected successful post-invalidation walk, got {other:?}"),
            },
            TranslationProbeCacheOutcome::Hit(_) => panic!("invalidation must force a walk"),
        };
        assert!(
            matches!(
                cache.probe(&paging, linear, AccessKind::Read, ring0),
                TranslationProbeCacheOutcome::Hit(_)
            ),
            "lookup after the forced walk must hit"
        );
        assert_eq!(first, after);
        assert_eq!(
            [ROOT, PDPT, PD + 16, PT]
                .map(|address| memory.read_u64(GuestPhysicalAddress::new(address)).unwrap()),
            table_snapshot,
        );

        // Repeat with an RBX target that is mapped in this snapshot. INVLPG
        // still performs no target translation or data callback.
        cpu.rip = linear.get() + 4;
        cpu.gpr[Register::Rbx as usize] = LINEAR_PAGE + 0x345;
        fetch_only.fetches.clear();
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, ring0),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        let mapped_result = step_invlpg(
            InvlpgLong64V1,
            &mut cpu,
            &mut fetch_only,
            ring0,
            0xd00,
            od_isa::INTEGER_FLAGS,
        );
        assert_eq!(
            fetch_only.fetches,
            [linear.get() + 4, linear.get() + 5, linear.get() + 6]
        );
        assert_eq!(
            mapped_result.exit,
            InvlpgExit::TranslationInvalidation(TranslationInvalidation::Invlpg {
                linear_address: LINEAR_PAGE + 0x345,
            })
        );
        assert!(
            consume_translation_invalidation(
                &mut cache,
                match mapped_result.exit {
                    InvlpgExit::TranslationInvalidation(effect) => effect,
                    other => panic!("expected mapped-target effect, got {other:?}"),
                },
            )
            .consumed
        );
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, ring0),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, ring0),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        cache.clear();
        let empty_consumption = consume_translation_invalidation(
            &mut cache,
            TranslationInvalidation::Invlpg {
                linear_address: 0xdead_0000,
            },
        );
        assert_eq!(
            empty_consumption,
            super::TranslationInvalidationConsumption {
                consumed: true,
                removed_cached_entry: false
            }
        );
        assert!(cache.is_empty());
        assert_eq!(
            consume_translation_invalidation(
                &mut cache,
                TranslationInvalidation::Invlpg {
                    linear_address: 0xdead_1000,
                },
            ),
            super::TranslationInvalidationConsumption {
                consumed: true,
                removed_cached_entry: false,
            }
        );
    }

    #[test]
    fn nonretiring_or_noninvalidating_invlpg_paths_preserve_warmed_cache() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        for (offset, bytes) in [
            (0x20, &[0x0f, 0x01, 0x3b][..]),
            (0x30, &[0x0f, 0x01, 0x3b][..]),
            (0x40, &[0xf0, 0x0f, 0x01, 0x3b][..]),
            (0x50, &[0x0f, 0x01, 0x3b][..]),
        ] {
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(PHYSICAL_PAGE + offset),
                    bytes,
                    |_| {},
                )
                .unwrap();
        }
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let ring0 = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let ring1 = AccessContext {
            privilege: PrivilegeLevel::Ring1,
            ..ring0
        };
        let cache_key = GuestLinearAddress::new(LINEAR_PAGE + 0x10);
        let mut cache = TranslationProbeCache::new(&memory);
        assert!(matches!(
            cache.probe(&paging, cache_key, AccessKind::Read, ring0),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            cache.probe(&paging, cache_key, AccessKind::Read, ring0),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let mut run_case = |rip: u64, rbx: u64, context: AccessContext, fail_fetch_at| {
            assert!(matches!(
                cache.probe(&paging, cache_key, AccessKind::Read, ring0),
                TranslationProbeCacheOutcome::Hit(_)
            ));
            let mut cpu = LongModeState::default();
            cpu.rip = rip;
            cpu.gpr[Register::Rbx as usize] = rbx;
            cpu.cr0 = Some(0x8001_0033);
            cpu.cr3 = Some(ROOT);
            cpu.cr4 = Some(0x20);
            cpu.activity = Some(CpuActivity::Running);
            cpu.sti_shadow = Some(StiShadow::Clear);
            let mut fetch_only = FetchOnlySnapshot {
                memory: &memory,
                paging,
                fetches: Vec::new(),
                fail_fetch_at,
            };
            let result = step_invlpg(
                InvlpgLong64V1,
                &mut cpu,
                &mut fetch_only,
                context,
                0xd00,
                od_isa::INTEGER_FLAGS,
            );
            assert!(matches!(
                cache.probe(&paging, cache_key, AccessKind::Read, ring0),
                TranslationProbeCacheOutcome::Hit(_)
            ));
            result
        };

        let nop = run_case(LINEAR_PAGE + 0x20, 0x0000_8000_0000_0000, ring0, None);
        assert_eq!(nop.exit, InvlpgExit::Retired);
        let privilege_fault = run_case(LINEAR_PAGE + 0x30, 0x2000, ring1, None);
        assert!(matches!(
            privilege_fault.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault { vector: 13, .. })
        ));
        let lock_fault = run_case(LINEAR_PAGE + 0x40, 0x2000, ring0, None);
        assert!(matches!(
            lock_fault.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault { vector: 6, .. })
        ));
        let fetch_fault = run_case(LINEAR_PAGE + 0x50, 0x2000, ring0, Some(LINEAR_PAGE + 0x51));
        assert!(matches!(fetch_fault.exit, InvlpgExit::Backend(_)));
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

        cache.clear();
        assert!(cache.is_empty());
        assert!(matches!(
            cache.probe(&paging, second, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        let (hit, after_clear) =
            translated(cache.probe(&paging, second, AccessKind::Read, explicit_ring3()));
        assert!(hit);
        assert_eq!(after_clear.linear.get(), 0x0040_02a5);
        assert_eq!(after_clear.physical.get(), 0x0000_92a5);
        assert_eq!(after_clear.page_base.get(), PHYSICAL_PAGE);
        assert_eq!(after_clear.offset, 0x2a5);
        assert_eq!(after_clear.permissions, second_translation.permissions);
        assert_eq!(after_clear.leaf_entry_address.get(), PT);
        assert_eq!(after_clear.leaf_entry_value, PHYSICAL_PAGE | 7);
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
    fn failed_replacement_leaves_cache_empty_and_next_matching_request_walks() {
        let memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 9);
        let missing = GuestLinearAddress::new(LINEAR_PAGE + 0x1009);
        let mut cache = TranslationProbeCache::new(&memory);

        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        match cache.probe(&paging, missing, AccessKind::Read, explicit_ring3()) {
            TranslationProbeCacheOutcome::Walk(report) => {
                assert!(matches!(
                    report.outcome,
                    Ok(crate::ProbeOutcome::PageFault(fault)) if fault.error_code == 0x04
                ));
                assert_eq!(report.trace.len(), 4);
            }
            TranslationProbeCacheOutcome::Hit(value) => {
                panic!("failed replacement unexpectedly hit as {value:?}")
            }
        }
        assert!(cache.is_empty());
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Hit(_)
        ));
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
    fn empty_and_repeated_clear_leave_page_table_ram_unchanged() {
        let memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x33);
        let entries = [ROOT, PDPT, PD + 16, PT];
        let before: Vec<_> = entries
            .iter()
            .map(|address| {
                memory
                    .read_u64(GuestPhysicalAddress::new(*address))
                    .unwrap()
            })
            .collect();
        let mut cache = TranslationProbeCache::new(&memory);

        cache.clear();
        cache.clear();
        assert!(cache.is_empty());
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        cache.clear();
        cache.clear();
        assert!(cache.is_empty());

        let after: Vec<_> = entries
            .iter()
            .map(|address| {
                memory
                    .read_u64(GuestPhysicalAddress::new(*address))
                    .unwrap()
            })
            .collect();
        assert_eq!(after, before);
        assert!(after.iter().all(|entry| entry & ((1 << 5) | (1 << 6)) == 0));
    }

    #[test]
    fn dropping_cache_ends_borrow_so_page_table_can_be_remapped_before_recreation() {
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
        // The scope ends the immutable cache borrow; mutation is then allowed.
        let (hit, translation) =
            translated(replacement.probe(&paging, linear, AccessKind::Read, explicit_ring3()));
        assert!(!hit);
        assert_eq!(translation.physical.get(), 0xa033);
        let (hit, translation) =
            translated(replacement.probe(&paging, linear, AccessKind::Read, explicit_ring3()));
        assert!(hit);
        assert_eq!(translation.physical.get(), 0xa033);
        assert_eq!(translation.page_base.get(), 0xa000);
        assert_eq!(translation.leaf_entry_address.get(), PT);
        assert_eq!(translation.leaf_entry_value, 0xa000 | 7);
    }

    #[test]
    fn leaf_write_permission_tighten_and_relax_are_observed_across_cache_lifetimes() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);

        {
            let mut cache = TranslationProbeCache::new(&memory);
            let (hit, translated) =
                translated(cache.probe(&paging, linear, AccessKind::Write, explicit_ring3()));
            assert!(!hit);
            assert_eq!(translated.physical.get(), 0x9051);
            assert!(matches!(
                cache.probe(&paging, linear, AccessKind::Write, explicit_ring3()),
                TranslationProbeCacheOutcome::Hit(_)
            ));
        }
        memory
            .write_u64(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 5, |_| {})
            .unwrap();
        {
            let mut cache = TranslationProbeCache::new(&memory);
            match cache.probe(&paging, linear, AccessKind::Write, explicit_ring3()) {
                TranslationProbeCacheOutcome::Walk(report) => assert!(matches!(
                    report.outcome,
                    Ok(crate::ProbeOutcome::PageFault(fault)) if fault.error_code == 0x07
                )),
                TranslationProbeCacheOutcome::Hit(value) => {
                    panic!("write-protected mapping hit as {value:?}")
                }
            }
            assert!(cache.is_empty());
        }
        memory
            .write_u64(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 7, |_| {})
            .unwrap();
        let mut cache = TranslationProbeCache::new(&memory);
        let (hit, translated) =
            translated(cache.probe(&paging, linear, AccessKind::Write, explicit_ring3()));
        assert!(!hit);
        assert_eq!(translated.physical.get(), 0x9051);
        assert_eq!(translated.leaf_entry_value, PHYSICAL_PAGE | 7);
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Write, explicit_ring3()),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn nx_fetch_rights_are_observed_across_cache_lifetimes_with_exact_pfec() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x62);

        {
            let mut cache = TranslationProbeCache::new(&memory);
            let (hit, translation) =
                translated(cache.probe(&paging, linear, AccessKind::Fetch, explicit_ring3()));
            assert!(!hit);
            assert_eq!(translation.physical.get(), 0x9062);
            assert!(matches!(
                cache.probe(&paging, linear, AccessKind::Fetch, explicit_ring3()),
                TranslationProbeCacheOutcome::Hit(_)
            ));
        }
        memory
            .write_u64(
                GuestPhysicalAddress::new(PT),
                PHYSICAL_PAGE | 7 | (1 << 63),
                |_| {},
            )
            .unwrap();
        {
            let mut cache = TranslationProbeCache::new(&memory);
            match cache.probe(&paging, linear, AccessKind::Fetch, explicit_ring3()) {
                TranslationProbeCacheOutcome::Walk(report) => assert!(matches!(
                    report.outcome,
                    Ok(crate::ProbeOutcome::PageFault(fault)) if fault.error_code == 0x15
                )),
                TranslationProbeCacheOutcome::Hit(value) => {
                    panic!("NX mapping hit as {value:?}")
                }
            }
            assert!(cache.is_empty());
        }
        memory
            .write_u64(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 7, |_| {})
            .unwrap();
        let mut cache = TranslationProbeCache::new(&memory);
        let (hit, translation) =
            translated(cache.probe(&paging, linear, AccessKind::Fetch, explicit_ring3()));
        assert!(!hit);
        assert_eq!(translation.physical.get(), 0x9062);
        assert_eq!(translation.leaf_entry_value, PHYSICAL_PAGE | 7);
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Fetch, explicit_ring3()),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn cache_probe_has_no_write_callbacks_or_page_table_ad_side_effects() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let paging = config(0x8001_0011, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x74);
        let mut writes = Vec::new();
        memory
            .write_u64(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 7, |range| {
                writes.push((range.start().get(), range.len()));
            })
            .unwrap();
        assert_eq!(writes, [(PT, 8)]);
        writes.clear();
        let entries = [ROOT, PDPT, PD + 16, PT];
        let before: Vec<_> = entries
            .iter()
            .map(|address| {
                memory
                    .read_u64(GuestPhysicalAddress::new(*address))
                    .unwrap()
            })
            .collect();

        let mut cache = TranslationProbeCache::new(&memory);
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            cache.probe(&paging, linear, AccessKind::Read, explicit_ring3()),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        assert!(
            writes.is_empty(),
            "probe invoked a committed-write callback"
        );
        let after: Vec<_> = entries
            .iter()
            .map(|address| {
                memory
                    .read_u64(GuestPhysicalAddress::new(*address))
                    .unwrap()
            })
            .collect();
        assert_eq!(after, before);
        assert!(after.iter().all(|entry| entry & ((1 << 5) | (1 << 6)) == 0));
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
