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
use crate::{GuestPhysicalAddress, GuestPhysicalRange, MemoryAccessError, PhysicalMemory};
use od_isa::TranslationInvalidation;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_isa::{
    InvlpgExit, InvlpgLong64V1, InvlpgResult, InvlpgWpLong64V1, LongModeState, MovCr0WpLong64V1,
    MovCr3Exit, MovCr3InputError, MovCr3Long64V1, MovCr3Result, OperandSize, PagedProbeExit,
    PagedProbeResult, TranslationContextChange, step_invlpg, step_invlpg_wp, step_mov_cr0_wp,
    step_mov_cr3,
};

const PHYSICAL_ADDRESS_LIMIT: u128 = 1u128 << 36;

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
    cache.state.consume(effect)
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

#[derive(Default)]
struct TranslationProbeCacheState {
    entry: Option<(CacheTag, CachedTranslation)>,
}

impl TranslationProbeCacheState {
    fn clear(&mut self) {
        self.entry = None;
    }

    fn is_empty(&self) -> bool {
        self.entry.is_none()
    }

    fn consume(&mut self, effect: TranslationInvalidation) -> TranslationInvalidationConsumption {
        match effect {
            TranslationInvalidation::Invlpg { .. } => {
                let removed_cached_entry = !self.is_empty();
                self.clear();
                TranslationInvalidationConsumption {
                    consumed: true,
                    removed_cached_entry,
                }
            }
        }
    }

    fn probe(
        &mut self,
        memory: &PhysicalMemory,
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
        self.entry = None;
        let report = probe_long64_4k(memory, paging, linear, access, context);
        if let Ok(ProbeOutcome::Translated(translation)) = report.outcome {
            self.entry = Some((tag, CachedTranslation::from_translation(translation)));
        }
        TranslationProbeCacheOutcome::Walk(report)
    }
}

/// Experimental capacity-one cache bound to one immutable physical-memory
/// snapshot. The complete tag is compared on every lookup.
pub struct TranslationProbeCache<'a> {
    memory: &'a PhysicalMemory,
    state: TranslationProbeCacheState,
}

impl<'a> TranslationProbeCache<'a> {
    pub fn new(memory: &'a PhysicalMemory) -> Self {
        Self {
            memory,
            state: TranslationProbeCacheState::default(),
        }
    }

    pub fn clear(&mut self) {
        self.state.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    pub fn probe(
        &mut self,
        paging: &Long64ProbeConfig,
        linear: GuestLinearAddress,
        access: AccessKind,
        context: AccessContext,
    ) -> TranslationProbeCacheOutcome {
        self.state
            .probe(self.memory, paging, linear, access, context)
    }
}

/// Errors while constructing or using the bounded mutable translation session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutableTranslationSessionError {
    SessionPoisoned,
    UnsupportedPagingProfile,
    Cr3Mismatch {
        expected: u64,
        actual: Option<u64>,
    },
    MovCr3Input(MovCr3InputError),
    Cr3RetiredSessionStateMismatch {
        effect: TranslationContextChange,
        cpu_cr3: Option<u64>,
        instructions_executed: u64,
    },
    Cr3RetiredInvalidSnapshot {
        effect: TranslationContextChange,
        source: crate::page_walk::ProbeConfigError,
    },
    Cr0RetiredSessionStateMismatch {
        effect: TranslationContextChange,
        cpu_cr0: Option<u64>,
        cpu_cr3: Option<u64>,
        cpu_cr4: Option<u64>,
        instructions_executed: u64,
    },
    Cr0RetiredWithoutEffect {
        exit: PagedProbeExit<std::convert::Infallible>,
        instructions_executed: u64,
    },
    Cr0RetiredInvalidSnapshot {
        effect: TranslationContextChange,
        source: crate::page_walk::ProbeConfigError,
    },
    UnalignedPteWrite {
        address: GuestPhysicalAddress,
    },
    PhysicalAddressWidth {
        address: GuestPhysicalAddress,
    },
    Memory(MemoryAccessError),
}

/// One-vCPU diagnostic session over the sole mutable owner of guest RAM.
/// Successful pure translations are cached as values; no references into RAM
/// or cache entries escape the session.
pub struct MutableTranslationProbeSession<'a> {
    memory: &'a mut PhysicalMemory,
    paging: Long64ProbeConfig,
    state: TranslationProbeCacheState,
    poisoned: bool,
}

impl<'a> MutableTranslationProbeSession<'a> {
    pub fn new(
        memory: &'a mut PhysicalMemory,
        paging: Long64ProbeConfig,
    ) -> Result<Self, MutableTranslationSessionError> {
        if !matches!(paging.cr0(), 0x8001_0033 | 0x8000_0033)
            || paging.cr4() != 0x20
            || paging.efer() != 0xd00
        {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }
        Ok(Self {
            memory,
            paging,
            state: TranslationProbeCacheState::default(),
            poisoned: false,
        })
    }

    pub fn probe(
        &mut self,
        linear: GuestLinearAddress,
        access: AccessKind,
        context: AccessContext,
    ) -> Result<TranslationProbeCacheOutcome, MutableTranslationSessionError> {
        if self.poisoned {
            return Err(MutableTranslationSessionError::SessionPoisoned);
        }
        Ok(self
            .state
            .probe(self.memory, &self.paging, linear, access, context))
    }

    /// Commit one aligned guest physical PTE-sized write. The write does not
    /// itself invalidate cached translations: a matching probe may still hit
    /// its resident stale entry until a retired INVLPG, MOV CR3, or MOV CR0.WP
    /// effect is consumed synchronously.
    pub fn write_pte(
        &mut self,
        address: GuestPhysicalAddress,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MutableTranslationSessionError> {
        self.ensure_usable()?;
        if address.get() & 7 != 0 {
            return Err(MutableTranslationSessionError::UnalignedPteWrite { address });
        }
        let range_end = u128::from(address.get()) + std::mem::size_of::<u64>() as u128;
        if range_end > PHYSICAL_ADDRESS_LIMIT {
            return Err(MutableTranslationSessionError::PhysicalAddressWidth { address });
        }
        self.memory
            .write_u64(address, value, committed_write)
            .map_err(MutableTranslationSessionError::Memory)
    }

    /// Fetches INVLPG bytes through a pure walker that bypasses the resident
    /// cache, then consumes a retired invalidation before returning.
    pub fn execute_invlpg(
        &mut self,
        cpu: &mut LongModeState,
        context: AccessContext,
        initial_rflags_defined: u64,
    ) -> Result<InvlpgResult, MutableTranslationSessionError> {
        self.ensure_usable()?;
        if self.paging.cr0() != 0x8001_0033 {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }
        if cpu.cr3 != Some(self.paging.cr3()) {
            return Err(MutableTranslationSessionError::Cr3Mismatch {
                expected: self.paging.cr3(),
                actual: cpu.cr3,
            });
        }
        if cpu.cr0 != Some(self.paging.cr0()) || cpu.cr4 != Some(self.paging.cr4()) {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }
        let result = {
            let mut fetch = SessionFetchMemory {
                memory: self.memory,
                paging: self.paging,
            };
            step_invlpg(
                InvlpgLong64V1,
                cpu,
                &mut fetch,
                context,
                self.paging.efer(),
                initial_rflags_defined,
            )
        };
        if let InvlpgExit::TranslationInvalidation(effect) = &result.exit {
            self.state.consume(*effect);
        }
        Ok(result)
    }

    /// Execute exact INVLPG under either bounded CR0.WP setting. Fetch uses a
    /// pure walk outside the resident cache; a canonical retired effect
    /// clears the single entry before this method returns. The noncanonical
    /// successful NOP intentionally leaves the cache resident.
    pub fn execute_invlpg_wp(
        &mut self,
        cpu: &mut LongModeState,
        context: AccessContext,
        initial_rflags_defined: u64,
    ) -> Result<InvlpgResult, MutableTranslationSessionError> {
        self.ensure_usable()?;
        if cpu.cr3 != Some(self.paging.cr3()) {
            return Err(MutableTranslationSessionError::Cr3Mismatch {
                expected: self.paging.cr3(),
                actual: cpu.cr3,
            });
        }
        if cpu.cr0 != Some(self.paging.cr0()) || cpu.cr4 != Some(self.paging.cr4()) {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }
        let result = {
            let mut fetch = SessionFetchMemory {
                memory: self.memory,
                paging: self.paging,
            };
            step_invlpg_wp(
                InvlpgWpLong64V1,
                cpu,
                &mut fetch,
                context,
                self.paging.efer(),
                initial_rflags_defined,
            )
        };
        if let InvlpgExit::TranslationInvalidation(effect) = &result.exit {
            self.state.consume(*effect);
        }
        Ok(result)
    }

    /// Execute one exact MOV CR3,RAX and consume its context-change effect
    /// before another translation lookup can observe the old cache entry.
    ///
    /// If the CPU instruction retires but its effect cannot be reconciled
    /// with this session, the error reports that boundary, clears the cache,
    /// retains the old paging snapshot, and poisons the session. CPU state is
    /// already committed; the caller must not use this session again.
    pub fn execute_mov_cr3(
        &mut self,
        cpu: &mut LongModeState,
        context: AccessContext,
        initial_rflags_defined: u64,
    ) -> Result<MovCr3Result, MutableTranslationSessionError> {
        self.ensure_usable()?;
        if self.paging.cr0() != 0x8001_0033 {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }
        if cpu.cr3 != Some(self.paging.cr3()) {
            return Err(MutableTranslationSessionError::Cr3Mismatch {
                expected: self.paging.cr3(),
                actual: cpu.cr3,
            });
        }
        if cpu.cr0 != Some(self.paging.cr0()) || cpu.cr4 != Some(self.paging.cr4()) {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }

        let result = {
            let mut fetch = SessionFetchMemory {
                memory: self.memory,
                paging: self.paging,
            };
            step_mov_cr3(
                MovCr3Long64V1,
                cpu,
                &mut fetch,
                context,
                self.paging.efer(),
                initial_rflags_defined,
            )
            .map_err(MutableTranslationSessionError::MovCr3Input)?
        };

        let effect = match &result.exit {
            MovCr3Exit::TranslationContextChange(effect) => *effect,
            _ => return Ok(result),
        };
        self.consume_retired_cr3_effect(effect, cpu.cr3, result.instructions_executed)?;
        Ok(result)
    }

    /// Execute one standalone MOV CR0, RAX WP toggle and consume its effect
    /// before another translation lookup. The diagnostic cache is cleared for
    /// every accepted write, including a same-value write.
    pub fn execute_mov_cr0_wp(
        &mut self,
        cpu: &mut LongModeState,
        context: AccessContext,
        initial_rflags_defined: u64,
    ) -> Result<PagedProbeResult<std::convert::Infallible>, MutableTranslationSessionError> {
        self.ensure_usable()?;
        if cpu.cr0 != Some(self.paging.cr0())
            || cpu.cr3 != Some(self.paging.cr3())
            || cpu.cr4 != Some(self.paging.cr4())
        {
            return Err(MutableTranslationSessionError::UnsupportedPagingProfile);
        }
        let result = {
            let mut fetch = SessionFetchMemory {
                memory: self.memory,
                paging: self.paging,
            };
            step_mov_cr0_wp(
                MovCr0WpLong64V1,
                cpu,
                &mut fetch,
                context,
                self.paging.efer(),
                initial_rflags_defined,
            )
        };
        self.consume_retired_cr0_result(&result, cpu)?;
        Ok(result)
    }

    fn consume_retired_cr0_result(
        &mut self,
        result: &PagedProbeResult<std::convert::Infallible>,
        cpu: &LongModeState,
    ) -> Result<(), MutableTranslationSessionError> {
        if let PagedProbeExit::TranslationContextChange(effect) = &result.exit {
            return self.consume_retired_cr0_effect(
                *effect,
                cpu.cr0,
                cpu.cr3,
                cpu.cr4,
                result.instructions_executed,
            );
        }
        if matches!(result.exit, PagedProbeExit::Retired) || result.instructions_executed != 0 {
            self.poison();
            return Err(MutableTranslationSessionError::Cr0RetiredWithoutEffect {
                exit: result.exit.clone(),
                instructions_executed: result.instructions_executed,
            });
        }
        Ok(())
    }

    fn consume_retired_cr0_effect(
        &mut self,
        effect: TranslationContextChange,
        cpu_cr0: Option<u64>,
        cpu_cr3: Option<u64>,
        cpu_cr4: Option<u64>,
        instructions_executed: u64,
    ) -> Result<(), MutableTranslationSessionError> {
        let new_cr0 = match effect {
            TranslationContextChange::Cr0Write { new_cr0, .. } => new_cr0,
            _ => {
                self.poison();
                return Err(
                    MutableTranslationSessionError::Cr0RetiredSessionStateMismatch {
                        effect,
                        cpu_cr0,
                        cpu_cr3,
                        cpu_cr4,
                        instructions_executed,
                    },
                );
            }
        };
        if instructions_executed != 1
            || effect
                != (TranslationContextChange::Cr0Write {
                    old_cr0: self.paging.cr0(),
                    new_cr0,
                })
            || cpu_cr0 != Some(new_cr0)
            || cpu_cr3 != Some(self.paging.cr3())
            || cpu_cr4 != Some(self.paging.cr4())
            || !matches!(new_cr0, 0x8001_0033 | 0x8000_0033)
        {
            self.poison();
            return Err(
                MutableTranslationSessionError::Cr0RetiredSessionStateMismatch {
                    effect,
                    cpu_cr0,
                    cpu_cr3,
                    cpu_cr4,
                    instructions_executed,
                },
            );
        }
        let replacement = Long64ProbeConfig::new(
            new_cr0,
            self.paging.cr3(),
            self.paging.cr4(),
            self.paging.efer(),
        )
        .map_err(|source| {
            self.poison();
            MutableTranslationSessionError::Cr0RetiredInvalidSnapshot { effect, source }
        })?;
        self.paging = replacement;
        self.state.clear();
        Ok(())
    }

    fn consume_retired_cr3_effect(
        &mut self,
        effect: TranslationContextChange,
        cpu_cr3: Option<u64>,
        instructions_executed: u64,
    ) -> Result<(), MutableTranslationSessionError> {
        let new_cr3 = match effect {
            TranslationContextChange::Cr3Write { new_cr3, .. } => new_cr3,
            _ => {
                self.poison();
                return Err(
                    MutableTranslationSessionError::Cr3RetiredSessionStateMismatch {
                        effect,
                        cpu_cr3,
                        instructions_executed,
                    },
                );
            }
        };
        if instructions_executed != 1
            || effect
                != (TranslationContextChange::Cr3Write {
                    old_cr3: self.paging.cr3(),
                    new_cr3,
                })
            || cpu_cr3 != Some(new_cr3)
        {
            self.poison();
            return Err(
                MutableTranslationSessionError::Cr3RetiredSessionStateMismatch {
                    effect,
                    cpu_cr3,
                    instructions_executed,
                },
            );
        }

        let replacement = Long64ProbeConfig::new(
            self.paging.cr0(),
            new_cr3,
            self.paging.cr4(),
            self.paging.efer(),
        )
        .map_err(|source| {
            self.poison();
            MutableTranslationSessionError::Cr3RetiredInvalidSnapshot { effect, source }
        })?;

        self.paging = replacement;
        self.state.clear();
        Ok(())
    }

    fn ensure_usable(&self) -> Result<(), MutableTranslationSessionError> {
        if self.poisoned {
            Err(MutableTranslationSessionError::SessionPoisoned)
        } else {
            Ok(())
        }
    }

    fn poison(&mut self) {
        self.state.clear();
        self.poisoned = true;
    }

    pub fn is_cache_empty(&self) -> bool {
        self.state.is_empty()
    }
}

struct SessionFetchMemory<'a> {
    memory: &'a PhysicalMemory,
    paging: Long64ProbeConfig,
}

impl GuestMemory for SessionFetchMemory<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        let report = probe_long64_4k(
            self.memory,
            &self.paging,
            GuestLinearAddress::new(address),
            AccessKind::Fetch,
            context,
        );
        match report.outcome {
            Ok(ProbeOutcome::Translated(translation)) => self
                .memory
                .read_u8(translation.physical)
                .map_err(|error| MemoryError::Backend(format!("session fetch read: {error}"))),
            Ok(ProbeOutcome::PageFault(fault)) => Err(MemoryError::Architectural(
                od_isa::memory::ArchitecturalFault {
                    vector: 14,
                    error_code: Some(fault.error_code),
                    linear_address: Some(fault.linear.get()),
                },
            )),
            Err(error) => Err(MemoryError::Backend(format!(
                "session fetch translation: {error:?}"
            ))),
        }
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "session fetch adapter does not support data reads".into(),
        ))
    }

    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "session fetch adapter does not support data writes".into(),
        ))
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "session fetch adapter does not support atomics".into(),
        ))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "session fetch adapter does not support atomics".into(),
        ))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "session fetch adapter does not support atomics".into(),
        ))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "session fetch adapter does not support fences".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MutableTranslationProbeSession, MutableTranslationSessionError, TranslationProbeCache,
        TranslationProbeCacheOutcome, consume_translation_invalidation,
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
        CpuActivity, InvlpgExit, InvlpgLong64V1, LongModeState, MovCr3Exit, OperandSize,
        PagedProbeExit, PagedProbeResult, Register, StiShadow, TranslationContextChange,
        TranslationInvalidation, step_invlpg,
    };

    const ROOT: u64 = 0x1000;
    const PDPT: u64 = 0x2000;
    const PD: u64 = 0x3000;
    const PT: u64 = 0x4000;
    const ROOT_B: u64 = 0x5000;
    const PDPT_B: u64 = 0x6000;
    const PD_B: u64 = 0x7000;
    const PT_B: u64 = 0x8000;
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

    fn invlpg_cpu(rip: u64, target: u64) -> LongModeState {
        let mut cpu = LongModeState::default();
        cpu.rip = rip;
        cpu.gpr[Register::Rbx as usize] = target;
        cpu.cr0 = Some(0x8001_0033);
        cpu.cr3 = Some(ROOT);
        cpu.cr4 = Some(0x20);
        cpu.activity = Some(CpuActivity::Running);
        cpu.sti_shadow = Some(StiShadow::Clear);
        cpu
    }

    fn install_root_b(memory: &mut PhysicalMemory, leaf: u64) {
        for (address, value) in [
            (ROOT_B, PDPT_B | 7),
            (PDPT_B, PD_B | 7),
            (PD_B + 16, PT_B | 7),
            (PT_B, leaf),
        ] {
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
    }

    fn mov_cr3_cpu(rip: u64, new_cr3: u64) -> LongModeState {
        let mut cpu = LongModeState::default();
        cpu.rip = rip;
        cpu.gpr[Register::Rax as usize] = new_cr3;
        cpu.cr0 = Some(0x8001_0033);
        cpu.cr3 = Some(ROOT);
        cpu.cr4 = Some(0x20);
        cpu.activity = Some(CpuActivity::Running);
        cpu.sti_shadow = Some(StiShadow::Clear);
        cpu
    }

    fn mov_cr3_context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn mov_cr0_cpu(rip: u64, new_cr0: u64) -> LongModeState {
        let mut cpu = LongModeState::default();
        cpu.rip = rip;
        cpu.gpr[Register::Rax as usize] = new_cr0;
        cpu.cr0 = Some(0x8001_0033);
        cpu.cr3 = Some(ROOT);
        cpu.cr4 = Some(0x20);
        cpu.activity = Some(CpuActivity::Running);
        cpu.sti_shadow = Some(StiShadow::Clear);
        cpu
    }

    #[test]
    fn mutable_session_remap_and_permission_refresh_require_retired_invlpg() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let instruction = [0x0f, 0x01, 0x3b];
        // Only the replacement physical page contains INVLPG. The stale
        // cached fetch translation still points at zero-filled old RAM.
        memory
            .write_bytes(
                GuestPhysicalAddress::new(0xa000 + 0x20),
                &instruction,
                |_| {},
            )
            .unwrap();
        memory
            .write_u64(GuestPhysicalAddress::new(PT + 8), 0xb000 | 7, |_| {})
            .unwrap();
        memory
            .write_bytes(
                GuestPhysicalAddress::new(0xb000 + 0x20),
                &instruction,
                |_| {},
            )
            .unwrap();
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let ring0 = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let ring3 = AccessContext::default();
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
        let invlpg_rip = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        let mut writes = Vec::new();
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();

        let (hit, initial) = translated(session.probe(linear, AccessKind::Read, ring3).unwrap());
        assert!(!hit);
        assert_eq!(initial.physical.get(), 0x9051);
        assert!(matches!(
            session.probe(linear, AccessKind::Read, ring3).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let (fetch_hit, old_fetch) =
            translated(session.probe(invlpg_rip, AccessKind::Fetch, ring0).unwrap());
        assert!(!fetch_hit);
        assert_eq!(old_fetch.physical.get(), PHYSICAL_PAGE + 0x20);
        assert!(matches!(
            session.probe(invlpg_rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 7, |range| {
                writes.push((range.start().get(), range.len()));
            })
            .unwrap();
        assert_eq!(writes, [(PT, 8)]);
        assert!(
            matches!(
                session.probe(invlpg_rip, AccessKind::Fetch, ring0).unwrap(),
                TranslationProbeCacheOutcome::Hit(ref value) if value.physical.get() == PHYSICAL_PAGE + 0x20
            ),
            "PTE writes retain the stale instruction-fetch translation until INVLPG"
        );

        let mut cpu = invlpg_cpu(invlpg_rip.get(), LINEAR_PAGE + 0x80);
        let result = session
            .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert!(matches!(
            result.exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        assert!(
            session.is_cache_empty(),
            "retired effect is consumed synchronously"
        );
        let (fetch_hit, new_fetch) =
            translated(session.probe(invlpg_rip, AccessKind::Fetch, ring0).unwrap());
        assert!(!fetch_hit, "post-INVLPG instruction lookup must walk");
        assert_eq!(new_fetch.physical.get(), 0xa020);
        assert!(matches!(
            session.probe(invlpg_rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        let (hit, remapped) = translated(session.probe(linear, AccessKind::Read, ring3).unwrap());
        assert!(!hit, "the first lookup after INVLPG must walk");
        assert_eq!(remapped.physical.get(), 0xa051);
        assert_eq!(remapped.page_base.get(), 0xa000);
        assert!(matches!(
            session.probe(linear, AccessKind::Read, ring3).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        // Tighten write permission without an implicit flush, then observe the
        // exact protection fault only after INVLPG clears the entry.
        let write_linear = GuestLinearAddress::new(LINEAR_PAGE + 0x52);
        assert!(matches!(
            session
                .probe(write_linear, AccessKind::Write, ring3)
                .unwrap(),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            session
                .probe(write_linear, AccessKind::Write, ring3)
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 5, |_| {})
            .unwrap();
        assert!(matches!(
            session
                .probe(write_linear, AccessKind::Write, ring3)
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        let mut cpu = invlpg_cpu(LINEAR_PAGE + 0x20, write_linear.get());
        assert!(matches!(
            session
                .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        match session
            .probe(write_linear, AccessKind::Write, ring3)
            .unwrap()
        {
            TranslationProbeCacheOutcome::Walk(report) => assert!(matches!(
                report.outcome,
                Ok(crate::ProbeOutcome::PageFault(fault)) if fault.error_code == 0x07
            )),
            TranslationProbeCacheOutcome::Hit(value) => {
                panic!("write protection change was stale after INVLPG: {value:?}")
            }
        }
        assert!(
            session.is_cache_empty(),
            "faulting mappings are never cached"
        );

        // Relax the permission and refresh again; the fresh mapping walks,
        // then becomes a hit with the updated leaf value.
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 7, |_| {})
            .unwrap();
        let mut cpu = invlpg_cpu(LINEAR_PAGE + 0x20, write_linear.get());
        assert!(matches!(
            session
                .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        let (hit, relaxed) = translated(
            session
                .probe(write_linear, AccessKind::Write, ring3)
                .unwrap(),
        );
        assert!(!hit);
        assert_eq!(relaxed.physical.get(), 0xa052);
        assert_eq!(relaxed.leaf_entry_value, 0xa000 | 7);
        assert!(matches!(
            session
                .probe(write_linear, AccessKind::Write, ring3)
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        assert_eq!(writes, [(PT, 8)]);

        // User/supervisor tightening and relaxation are refreshed through the
        // same session. INVLPG code is mapped on a separate guest page so the
        // permission under test does not block its own instruction fetch.
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 3, |_| {})
            .unwrap();
        let mut cpu = invlpg_cpu(LINEAR_PAGE + 0x1020, write_linear.get());
        assert!(matches!(
            session
                .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        match session
            .probe(write_linear, AccessKind::Read, ring3)
            .unwrap()
        {
            TranslationProbeCacheOutcome::Walk(report) => assert!(matches!(
                report.outcome,
                Ok(crate::ProbeOutcome::PageFault(fault)) if fault.error_code == 0x05
            )),
            TranslationProbeCacheOutcome::Hit(value) => {
                panic!("supervisor-only mapping was stale after INVLPG: {value:?}")
            }
        }
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 5, |_| {})
            .unwrap();
        let mut cpu = invlpg_cpu(LINEAR_PAGE + 0x1020, write_linear.get());
        assert!(matches!(
            session
                .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        let (hit, user_mapping) = translated(
            session
                .probe(write_linear, AccessKind::Read, ring3)
                .unwrap(),
        );
        assert!(!hit);
        assert!(user_mapping.permissions.user);
        assert!(matches!(
            session
                .probe(write_linear, AccessKind::Read, ring3)
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        // NX changes are refreshed as well. The fetch instruction resides on
        // the second, executable page and is unaffected by the first PTE.
        session
            .probe(
                GuestLinearAddress::new(LINEAR_PAGE + 0x70),
                AccessKind::Fetch,
                ring3,
            )
            .unwrap();
        session
            .write_pte(
                GuestPhysicalAddress::new(PT),
                0xa000 | 7 | (1 << 63),
                |_| {},
            )
            .unwrap();
        let mut cpu = invlpg_cpu(LINEAR_PAGE + 0x1020, write_linear.get());
        assert!(matches!(
            session
                .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        match session
            .probe(
                GuestLinearAddress::new(LINEAR_PAGE + 0x70),
                AccessKind::Fetch,
                ring3,
            )
            .unwrap()
        {
            TranslationProbeCacheOutcome::Walk(report) => assert!(matches!(
                report.outcome,
                Ok(crate::ProbeOutcome::PageFault(fault)) if fault.error_code == 0x15
            )),
            TranslationProbeCacheOutcome::Hit(value) => {
                panic!("NX fetch denial was stale after INVLPG: {value:?}")
            }
        }
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 7, |_| {})
            .unwrap();
        let mut cpu = invlpg_cpu(LINEAR_PAGE + 0x1020, write_linear.get());
        assert!(matches!(
            session
                .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        let (hit, executable) = translated(
            session
                .probe(
                    GuestLinearAddress::new(LINEAR_PAGE + 0x70),
                    AccessKind::Fetch,
                    ring3,
                )
                .unwrap(),
        );
        assert!(!hit);
        assert!(!executable.permissions.execute_disabled);
        assert!(matches!(
            session
                .probe(
                    GuestLinearAddress::new(LINEAR_PAGE + 0x70),
                    AccessKind::Fetch,
                    ring3
                )
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn mutable_session_rejects_pte_range_outside_physical36_without_commit() {
        let high_base = 1u64 << 36;
        let low = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x20_000).unwrap();
        let high = GuestPhysicalRange::new(GuestPhysicalAddress::new(high_base), 0x1000).unwrap();
        let map = MachineMemoryMap::new(
            37,
            vec![
                MemoryRegionSpec::new(low, MemoryRegionKind::Ram),
                MemoryRegionSpec::new(high, MemoryRegionKind::Ram),
            ],
        )
        .unwrap();
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x21_000)).unwrap();
        let initial = 0xfeed_face_cafe_beef;
        memory
            .write_u64(GuestPhysicalAddress::new(high_base), initial, |_| {})
            .unwrap();

        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let mut callback_count = 0;
        {
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
            assert!(matches!(
                session.write_pte(
                    GuestPhysicalAddress::new(high_base),
                    0x1234,
                    |_| callback_count += 1
                ),
                Err(MutableTranslationSessionError::PhysicalAddressWidth { address })
                    if address.get() == high_base
            ));
        }
        assert_eq!(callback_count, 0);
        assert_eq!(
            memory
                .read_u64(GuestPhysicalAddress::new(high_base))
                .unwrap(),
            initial,
            "the out-of-profile RAM bytes must remain unchanged"
        );
    }

    #[test]
    fn invalid_pte_writes_are_checked_after_borrow_before_any_restore() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let mut before = [0; 16];
        memory
            .read_bytes(GuestPhysicalAddress::new(PT), &mut before)
            .unwrap();
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let mut callback_count = 0;
        {
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
            assert!(matches!(
                session.write_pte(GuestPhysicalAddress::new(PT + 1), 0, |_| callback_count +=
                    1),
                Err(MutableTranslationSessionError::UnalignedPteWrite { .. })
            ));
            assert!(matches!(
                session.write_pte(GuestPhysicalAddress::new(0x30_000), 0, |_| {
                    callback_count += 1
                }),
                Err(MutableTranslationSessionError::Memory(_))
            ));
        }

        let mut after = [0; 16];
        memory
            .read_bytes(GuestPhysicalAddress::new(PT), &mut after)
            .unwrap();
        assert_eq!(after, before, "inspect RAM after the mutable borrow ends");
        assert_eq!(callback_count, 0);
    }

    #[test]
    fn mutable_session_preserves_cache_on_invlpg_nop_faults_fetch_failure_and_bad_writes() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        for (offset, bytes) in [
            (0x20, &[0x0f, 0x01, 0x3b][..]),
            (0x30, &[0x0f, 0x01, 0x3b][..]),
            (0x40, &[0xf0, 0x0f, 0x01, 0x3b][..]),
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
        let key = GuestLinearAddress::new(LINEAR_PAGE + 0x10);
        let mut writes = Vec::new();
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let mut wrong_cr3 = invlpg_cpu(LINEAR_PAGE + 0x20, 0);
        wrong_cr3.cr3 = Some(ROOT + 0x1000);
        assert!(matches!(
            session.execute_invlpg(&mut wrong_cr3, ring0, od_isa::INTEGER_FLAGS),
            Err(MutableTranslationSessionError::Cr3Mismatch { .. })
        ));
        assert_eq!(wrong_cr3.rip, LINEAR_PAGE + 0x20);
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let failed = session.write_pte(GuestPhysicalAddress::new(0x30_000), 0, |range| {
            writes.push(range)
        });
        assert!(matches!(
            failed,
            Err(MutableTranslationSessionError::Memory(_))
        ));
        assert!(writes.is_empty(), "failed writes must not notify");
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        for (rip, target, context, expected) in [
            (LINEAR_PAGE + 0x20, 0x0000_8000_0000_0000, ring0, "nop"),
            (LINEAR_PAGE + 0x30, 0x2000, ring1, "privilege"),
            (LINEAR_PAGE + 0x40, 0x2000, ring0, "lock"),
        ] {
            let mut cpu = invlpg_cpu(rip, target);
            let result = session
                .execute_invlpg(&mut cpu, context, od_isa::INTEGER_FLAGS)
                .unwrap();
            match expected {
                "nop" => assert_eq!(result.exit, InvlpgExit::Retired),
                "privilege" => assert!(matches!(
                    result.exit,
                    InvlpgExit::Fault(od_isa::memory::ArchitecturalFault { vector: 13, .. })
                )),
                "lock" => assert!(matches!(
                    result.exit,
                    InvlpgExit::Fault(od_isa::memory::ArchitecturalFault { vector: 6, .. })
                )),
                _ => unreachable!(),
            }
            assert!(matches!(
                session.probe(key, AccessKind::Read, ring0).unwrap(),
                TranslationProbeCacheOutcome::Hit(_)
            ));
        }

        // Removing the code mapping makes the instruction fetch fail before
        // decode; the resident data translation remains intact.
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0, |_| {})
            .unwrap();
        let fetch_rip = LINEAR_PAGE + 0x20;
        let mut cpu = invlpg_cpu(fetch_rip, 0x2000);
        let fetch = session
            .execute_invlpg(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(
            fetch.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(fetch_rip),
            })
        );
        assert_eq!(fetch.instructions_executed, 0, "fetch fault cannot retire");
        assert_eq!(cpu.rip, fetch_rip, "fetch fault preserves the saved RIP");
        // Restore PTE through the same checked write path; neither operation
        // invalidates the cache on its own.
        session
            .write_pte(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 7, |_| {})
            .unwrap();
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn mov_cr3_switch_clears_cache_before_root_b_walk_and_hit() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &[0x0f, 0x22, 0xd8],
                |_| {},
            )
            .unwrap();
        install_root_b(&mut memory, 0xc000 | 7);
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();

        let (_, initial) = translated(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
        );
        assert_eq!(initial.physical.get(), PHYSICAL_PAGE + 0x51);
        assert!(matches!(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let mut cpu = mov_cr3_cpu(LINEAR_PAGE + 0x20, ROOT_B);
        let result = session
            .execute_mov_cr3(&mut cpu, mov_cr3_context(), od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            result.exit,
            MovCr3Exit::TranslationContextChange(TranslationContextChange::Cr3Write {
                old_cr3: ROOT,
                new_cr3: ROOT_B,
            })
        );
        assert_eq!(cpu.cr3, Some(ROOT_B));
        assert!(session.is_cache_empty());

        let (hit, after) = translated(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
        );
        assert!(!hit, "CR3 consumption must force a root-B walk");
        assert_eq!(after.physical.get(), 0xc051);
        assert!(matches!(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn same_cr3_write_invalidates_after_pte_remap() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &[0x0f, 0x22, 0xd8],
                |_| {},
            )
            .unwrap();
        memory
            .write_bytes(
                GuestPhysicalAddress::new(0xa020),
                &[0x0f, 0x22, 0xd8],
                |_| {},
            )
            .unwrap();
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();

        let (_, initial) = translated(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
        );
        assert_eq!(initial.physical.get(), PHYSICAL_PAGE + 0x51);
        assert!(matches!(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 5, |_| {})
            .unwrap();
        assert!(matches!(
            session.probe(linear, AccessKind::Read, explicit_ring3()).unwrap(),
            TranslationProbeCacheOutcome::Hit(ref value) if value.physical.get() == PHYSICAL_PAGE + 0x51
        ));

        let mut cpu = mov_cr3_cpu(LINEAR_PAGE + 0x20, ROOT);
        let result = session
            .execute_mov_cr3(&mut cpu, mov_cr3_context(), od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(cpu.cr3, Some(ROOT));
        assert!(session.is_cache_empty());

        let (hit, after) = translated(
            session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap(),
        );
        assert!(
            !hit,
            "writing the same CR3 value must still clear the cache"
        );
        assert_eq!(after.physical.get(), 0xa051);
        let TranslationProbeCacheOutcome::Walk(report) = session
            .probe(linear, AccessKind::Write, mov_cr3_context())
            .unwrap()
        else {
            panic!("newly denied write must walk the updated PTE");
        };
        let Ok(crate::ProbeOutcome::PageFault(fault)) = report.outcome else {
            panic!(
                "expected updated write protection, got {:?}",
                report.outcome
            );
        };
        assert_eq!(fault.linear, linear);
        assert_eq!(fault.error_code, 3);
    }

    #[test]
    fn mov_cr3_to_unusable_root_preserves_cpu_commit_and_returns_precise_probe_fault() {
        for (leaf, expected_error) in [(0, 0x10), (0xa000 | 7 | (1 << 63), 0x11)] {
            let mut memory = memory(PHYSICAL_PAGE | 7);
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                    &[0x0f, 0x22, 0xd8],
                    |_| {},
                )
                .unwrap();
            install_root_b(&mut memory, leaf);
            let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
            let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x40);
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
            let _ = session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap();

            let mut cpu = mov_cr3_cpu(LINEAR_PAGE + 0x20, ROOT_B);
            let result = session
                .execute_mov_cr3(&mut cpu, mov_cr3_context(), od_isa::INTEGER_FLAGS)
                .unwrap();
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(cpu.cr3, Some(ROOT_B));
            assert!(session.is_cache_empty());

            let TranslationProbeCacheOutcome::Walk(report) = session
                .probe(linear, AccessKind::Fetch, mov_cr3_context())
                .unwrap()
            else {
                panic!("faulting root-B fetch must walk after CR3 consumption");
            };
            let Ok(crate::ProbeOutcome::PageFault(fault)) = report.outcome else {
                panic!(
                    "expected precise root-B page fault, got {:?}",
                    report.outcome
                );
            };
            assert_eq!(fault.linear, linear);
            assert_eq!(fault.error_code, expected_error);
        }
    }

    #[test]
    fn mov_cr0_wp_switches_snapshot_clears_cache_and_changes_supervisor_write_result() {
        let mut memory = memory(PHYSICAL_PAGE | 5);
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &[0x0f, 0x22, 0xc0],
                |_| {},
            )
            .unwrap();
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x30),
                &[0x0f, 0x01, 0x3b],
                |_| {},
            )
            .unwrap();
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x40),
                &[0x0f, 0x22, 0xd8],
                |_| {},
            )
            .unwrap();
        let wp0 = 0x8000_0033;
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let ring0 = mov_cr3_context();
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x40);
        let instruction = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();

        // WP=1 denies a supervisor write to the read-only leaf.
        let TranslationProbeCacheOutcome::Walk(denied) =
            session.probe(linear, AccessKind::Write, ring0).unwrap()
        else {
            panic!("initial denied write must walk");
        };
        assert!(
            matches!(denied.outcome, Ok(crate::ProbeOutcome::PageFault(ref f)) if f.error_code == 3)
        );
        let _ = session.probe(linear, AccessKind::Read, ring0).unwrap();
        assert!(!session.is_cache_empty());

        let mut cpu = mov_cr0_cpu(instruction.get(), wp0);
        let table_addresses = [ROOT, PDPT, PD + 16, PT];
        let entries_before = table_addresses.map(|address| {
            session
                .memory
                .read_u64(GuestPhysicalAddress::new(address))
                .unwrap()
        });
        let result = session
            .execute_mov_cr0_wp(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        let entries_after = table_addresses.map(|address| {
            session
                .memory
                .read_u64(GuestPhysicalAddress::new(address))
                .unwrap()
        });
        assert_eq!(
            entries_after, entries_before,
            "MOV CR0 fetch/effect must not set A/D bits"
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(cpu.cr0, Some(wp0));
        assert!(
            session.is_cache_empty(),
            "effect must clear the cache immediately"
        );
        let TranslationProbeCacheOutcome::Walk(allowed) =
            session.probe(linear, AccessKind::Write, ring0).unwrap()
        else {
            panic!("first WP=0 write must walk");
        };
        assert!(matches!(
            allowed.outcome,
            Ok(crate::ProbeOutcome::Translated(_))
        ));
        let TranslationProbeCacheOutcome::Walk(ring3_denied) = session
            .probe(linear, AccessKind::Write, explicit_ring3())
            .unwrap()
        else {
            panic!("Ring3 write uses a distinct cache tag and must walk");
        };
        assert!(matches!(
            ring3_denied.outcome,
            Ok(crate::ProbeOutcome::PageFault(ref fault)) if fault.error_code == 7
        ));
        let _ = session.probe(linear, AccessKind::Read, ring0).unwrap();
        assert!(
            !session.is_cache_empty(),
            "successful Ring0 read must refill the cache before WP is restored"
        );

        // INVLPG and MOV CR3 profiles keep their original WP=1 admission.
        let mut rejected_cpu = cpu.clone();
        assert_eq!(
            session.execute_invlpg(&mut rejected_cpu, ring0, od_isa::INTEGER_FLAGS),
            Err(MutableTranslationSessionError::UnsupportedPagingProfile)
        );
        assert_eq!(
            session.execute_mov_cr3(&mut rejected_cpu, ring0, od_isa::INTEGER_FLAGS),
            Err(MutableTranslationSessionError::UnsupportedPagingProfile)
        );
        assert_eq!(rejected_cpu, cpu);

        let mut cpu = mov_cr0_cpu(instruction.get(), 0x8001_0033);
        cpu.cr0 = Some(wp0);
        let result = session
            .execute_mov_cr0_wp(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(cpu.cr0, Some(0x8001_0033));
        assert!(session.is_cache_empty());
        let TranslationProbeCacheOutcome::Walk(protected) =
            session.probe(linear, AccessKind::Write, ring0).unwrap()
        else {
            panic!("first WP=1 write must walk after effect");
        };
        assert!(
            matches!(protected.outcome, Ok(crate::ProbeOutcome::PageFault(ref f)) if f.error_code == 3)
        );

        // The prior profiles are admitted again after returning to WP=1.
        let mut invlpg = invlpg_cpu(LINEAR_PAGE + 0x30, LINEAR_PAGE + 0x80);
        assert!(matches!(
            session
                .execute_invlpg(&mut invlpg, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            InvlpgExit::TranslationInvalidation(_)
        ));
        let mut mov_cr3 = mov_cr3_cpu(LINEAR_PAGE + 0x40, ROOT);
        assert!(matches!(
            session
                .execute_mov_cr3(&mut mov_cr3, ring0, od_isa::INTEGER_FLAGS)
                .unwrap()
                .exit,
            MovCr3Exit::TranslationContextChange(TranslationContextChange::Cr3Write { .. })
        ));
    }

    #[test]
    fn same_value_mov_cr0_clears_stale_mapping_and_permission_cache() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        for physical in [PHYSICAL_PAGE, 0xa000] {
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(physical + 0x20),
                    &[0x0f, 0x22, 0xc0],
                    |_| {},
                )
                .unwrap();
        }
        let wp1 = 0x8001_0033;
        let paging = config(wp1, ROOT, 0x20, 0xd00);
        let ring0 = mov_cr3_context();
        let data = GuestLinearAddress::new(LINEAR_PAGE + 0x50);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
        let (hit, initial) = translated(session.probe(data, AccessKind::Write, ring0).unwrap());
        assert!(!hit);
        assert_eq!(initial.physical.get(), PHYSICAL_PAGE + 0x50);
        assert_eq!(initial.permissions.writable, true);
        assert!(matches!(
            session.probe(data, AccessKind::Write, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        session
            .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 5, |_| {})
            .unwrap();
        let (hit, stale) = translated(session.probe(data, AccessKind::Write, ring0).unwrap());
        assert!(hit, "PTE writes alone leave a matching entry resident");
        assert_eq!(stale.physical.get(), PHYSICAL_PAGE + 0x50);
        assert!(stale.permissions.writable);

        let rip = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        let mut cpu = mov_cr0_cpu(rip.get(), wp1);
        let effect = session
            .execute_mov_cr0_wp(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(effect.instructions_executed, 1);
        assert_eq!(cpu.cr0, Some(wp1));
        assert!(
            session.is_cache_empty(),
            "same-value effect clears synchronously"
        );
        let TranslationProbeCacheOutcome::Walk(updated) =
            session.probe(data, AccessKind::Write, ring0).unwrap()
        else {
            panic!("post-effect lookup must walk the changed PTE");
        };
        let Ok(crate::ProbeOutcome::PageFault(fault)) = updated.outcome else {
            panic!("same-value MOV CR0 must expose tightened write permission");
        };
        assert_eq!(fault.error_code, 3);
    }

    #[test]
    fn mov_cr0_fetch_fault_preserves_matching_ring0_fetch_cache_hit() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &[0x0f, 0x22, 0xc0],
                |_| {},
            )
            .unwrap();
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let ring0 = mov_cr3_context();
        let rip = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
        assert!(matches!(
            session.probe(rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            session.probe(rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0, |_| {})
            .unwrap();

        let mut cpu = mov_cr0_cpu(rip.get(), 0x8000_0033);
        let before = cpu.clone();
        let result = session
            .execute_mov_cr0_wp(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(
            result.exit,
            PagedProbeExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(rip.get()),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(session.paging.cr0(), 0x8001_0033);
        assert!(matches!(
            session.probe(rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn rejected_mov_cr3_paths_preserve_session_cache_and_snapshot() {
        fn check(
            instruction: &[u8],
            context: AccessContext,
            mismatch: bool,
            fetch_fault: bool,
            reserved_source: bool,
        ) {
            let mut memory = memory(PHYSICAL_PAGE | 7);
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                    instruction,
                    |_| {},
                )
                .unwrap();
            let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
            let read_linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
            let fetch_linear = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
            let cache_key = if fetch_fault {
                assert!(matches!(
                    session
                        .probe(fetch_linear, AccessKind::Fetch, mov_cr3_context())
                        .unwrap(),
                    TranslationProbeCacheOutcome::Walk(_)
                ));
                fetch_linear
            } else {
                assert!(matches!(
                    session
                        .probe(read_linear, AccessKind::Read, explicit_ring3())
                        .unwrap(),
                    TranslationProbeCacheOutcome::Walk(_)
                ));
                read_linear
            };
            let cached = if fetch_fault {
                session
                    .probe(fetch_linear, AccessKind::Fetch, mov_cr3_context())
                    .unwrap()
            } else {
                session
                    .probe(read_linear, AccessKind::Read, explicit_ring3())
                    .unwrap()
            };
            assert!(matches!(cached, TranslationProbeCacheOutcome::Hit(_)));
            if fetch_fault {
                session
                    .write_pte(GuestPhysicalAddress::new(PT), 0, |_| {})
                    .unwrap();
            }
            let mut cpu = mov_cr3_cpu(LINEAR_PAGE + 0x20, ROOT_B);
            if mismatch {
                cpu.cr3 = Some(ROOT_B);
            }
            if reserved_source {
                cpu.gpr[Register::Rax as usize] = ROOT_B | (1 << 40);
            }
            let before_cr3 = cpu.cr3;
            let before_rip = cpu.rip;
            let before_rax = cpu.gpr[Register::Rax as usize];
            let result = session.execute_mov_cr3(&mut cpu, context, od_isa::INTEGER_FLAGS);
            if mismatch {
                assert!(matches!(
                    result,
                    Err(MutableTranslationSessionError::Cr3Mismatch {
                        expected: ROOT,
                        actual: Some(ROOT_B),
                    })
                ));
            } else {
                let result = result.unwrap();
                assert_eq!(result.instructions_executed, 0);
                if instruction.first() == Some(&0xf0) {
                    assert_eq!(
                        result.exit,
                        MovCr3Exit::Fault(od_isa::memory::ArchitecturalFault {
                            vector: 6,
                            error_code: None,
                            linear_address: None,
                        }),
                        "LOCK MOV CR3 must raise #UD"
                    );
                } else if fetch_fault {
                    assert_eq!(
                        result.exit,
                        MovCr3Exit::Fault(od_isa::memory::ArchitecturalFault {
                            vector: 14,
                            error_code: Some(0x10),
                            linear_address: Some(fetch_linear.get()),
                        }),
                        "fetch fault must retain exact #PF details"
                    );
                } else if reserved_source {
                    assert_eq!(
                        result.exit,
                        MovCr3Exit::Fault(od_isa::memory::ArchitecturalFault {
                            vector: 13,
                            error_code: Some(0),
                            linear_address: None,
                        }),
                        "reserved source CR3 bits must raise #GP(0)"
                    );
                } else if context.privilege == PrivilegeLevel::Ring3 {
                    assert_eq!(
                        result.exit,
                        MovCr3Exit::Fault(od_isa::memory::ArchitecturalFault {
                            vector: 13,
                            error_code: Some(0),
                            linear_address: None,
                        })
                    );
                }
            }
            assert_eq!(cpu.cr3, before_cr3);
            assert_eq!(cpu.rip, before_rip);
            assert_eq!(cpu.gpr[Register::Rax as usize], before_rax);
            assert_eq!(session.paging.cr3(), ROOT, "session snapshot must not move");
            assert!(!session.is_cache_empty());
            let retained = if fetch_fault {
                session
                    .probe(fetch_linear, AccessKind::Fetch, mov_cr3_context())
                    .unwrap()
            } else {
                session
                    .probe(cache_key, AccessKind::Read, explicit_ring3())
                    .unwrap()
            };
            assert!(matches!(retained, TranslationProbeCacheOutcome::Hit(_)));
        }

        let ring0 = mov_cr3_context();
        check(&[0xf0, 0x0f, 0x22, 0xd8], ring0, false, false, false);
        check(
            &[0x0f, 0x22, 0xd8],
            AccessContext::default(),
            false,
            false,
            false,
        );
        check(&[0x0f, 0x22, 0xd8], ring0, false, true, false);
        check(&[0x0f, 0x22, 0xd8], ring0, false, false, true);
        check(&[0x0f, 0x22, 0xd8], ring0, true, false, false);
    }

    #[test]
    fn post_retirement_handoff_errors_poison_session_and_clear_cache() {
        fn exercise(
            effect: TranslationContextChange,
            cpu_cr3: Option<u64>,
            invalid_snapshot: bool,
        ) {
            let mut memory = memory(PHYSICAL_PAGE | 7);
            let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
            let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
            assert!(matches!(
                session
                    .probe(linear, AccessKind::Read, explicit_ring3())
                    .unwrap(),
                TranslationProbeCacheOutcome::Walk(_)
            ));
            assert!(matches!(
                session
                    .probe(linear, AccessKind::Read, explicit_ring3())
                    .unwrap(),
                TranslationProbeCacheOutcome::Hit(_)
            ));

            let error = session.consume_retired_cr3_effect(effect, cpu_cr3, 1);
            if invalid_snapshot {
                assert!(matches!(
                    error,
                    Err(MutableTranslationSessionError::Cr3RetiredInvalidSnapshot { .. })
                ));
            } else {
                assert!(matches!(
                    error,
                    Err(MutableTranslationSessionError::Cr3RetiredSessionStateMismatch { .. })
                ));
            }
            assert!(session.is_cache_empty(), "stale entry must be discarded");
            assert_eq!(session.paging.cr3(), ROOT, "old snapshot is retained");
            assert_eq!(
                session.probe(linear, AccessKind::Read, explicit_ring3()),
                Err(MutableTranslationSessionError::SessionPoisoned)
            );
            assert_eq!(
                session.write_pte(GuestPhysicalAddress::new(PT), 0, |_| {}),
                Err(MutableTranslationSessionError::SessionPoisoned)
            );
            assert_eq!(
                session.execute_invlpg(
                    &mut invlpg_cpu(LINEAR_PAGE + 0x20, linear.get()),
                    mov_cr3_context(),
                    od_isa::INTEGER_FLAGS,
                ),
                Err(MutableTranslationSessionError::SessionPoisoned)
            );
            assert_eq!(
                session.execute_mov_cr3(
                    &mut mov_cr3_cpu(LINEAR_PAGE + 0x20, ROOT_B),
                    mov_cr3_context(),
                    od_isa::INTEGER_FLAGS,
                ),
                Err(MutableTranslationSessionError::SessionPoisoned)
            );
        }

        exercise(
            TranslationContextChange::Cr3Write {
                old_cr3: ROOT_B,
                new_cr3: ROOT_B,
            },
            Some(ROOT_B),
            false,
        );
        exercise(
            TranslationContextChange::Cr3Write {
                old_cr3: ROOT,
                new_cr3: ROOT | (1 << 40),
            },
            Some(ROOT | (1 << 40)),
            true,
        );
        exercise(
            TranslationContextChange::Cr0Write {
                old_cr0: 0x8001_0033,
                new_cr0: 0x8001_0033,
            },
            Some(ROOT),
            false,
        );
    }

    #[test]
    fn unexpected_retired_cr0_effect_poisons_and_clears_session() {
        fn exercise(
            effect: TranslationContextChange,
            cpu_cr0: Option<u64>,
            cpu_cr3: Option<u64>,
            cpu_cr4: Option<u64>,
        ) {
            let mut memory = memory(PHYSICAL_PAGE | 7);
            let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
            let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
            let _ = session
                .probe(linear, AccessKind::Read, explicit_ring3())
                .unwrap();
            assert!(!session.is_cache_empty());

            assert!(matches!(
                session.consume_retired_cr0_effect(effect, cpu_cr0, cpu_cr3, cpu_cr4, 1),
                Err(MutableTranslationSessionError::Cr0RetiredSessionStateMismatch { .. })
            ));
            assert!(session.is_cache_empty());
            assert_eq!(
                session.probe(linear, AccessKind::Read, explicit_ring3()),
                Err(MutableTranslationSessionError::SessionPoisoned)
            );
        }

        exercise(
            TranslationContextChange::Cr3Write {
                old_cr3: ROOT,
                new_cr3: ROOT,
            },
            Some(0x8001_0033),
            Some(ROOT),
            Some(0x20),
        );
        exercise(
            TranslationContextChange::Cr0Write {
                old_cr0: 0x8000_0033,
                new_cr0: 0x8000_0033,
            },
            Some(0x8000_0033),
            Some(ROOT),
            Some(0x20),
        );
        exercise(
            TranslationContextChange::Cr0Write {
                old_cr0: 0x8001_0033,
                new_cr0: 0x8000_0033,
            },
            Some(0x8000_0033),
            Some(ROOT_B),
            Some(0x20),
        );
        exercise(
            TranslationContextChange::Cr0Write {
                old_cr0: 0x8001_0033,
                new_cr0: 0x8000_0033,
            },
            Some(0x8000_0033),
            Some(ROOT),
            Some(0x30),
        );

        let mut memory2 = memory(PHYSICAL_PAGE | 7);
        let mut session = MutableTranslationProbeSession::new(
            &mut memory2,
            config(0x8001_0033, ROOT, 0x20, 0xd00),
        )
        .unwrap();
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
        let _ = session
            .probe(linear, AccessKind::Read, explicit_ring3())
            .unwrap();
        let cpu = mov_cr0_cpu(LINEAR_PAGE, 0x8000_0033);
        let missing_effect = PagedProbeResult {
            exit: PagedProbeExit::Retired,
            instructions_executed: 1,
            rflags_defined: od_isa::INTEGER_FLAGS,
        };
        assert!(matches!(
            session.consume_retired_cr0_result(&missing_effect, &cpu),
            Err(MutableTranslationSessionError::Cr0RetiredWithoutEffect {
                instructions_executed: 1,
                ..
            })
        ));
        assert!(session.is_cache_empty());
        assert_eq!(
            session.probe(linear, AccessKind::Read, explicit_ring3()),
            Err(MutableTranslationSessionError::SessionPoisoned)
        );
    }

    #[test]
    fn retired_without_cr0_effect_poisoning_distinguishes_consistent_nonretirement() {
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x51);
        let cpu = mov_cr0_cpu(LINEAR_PAGE, 0x8000_0033);

        let mut memory_first = memory(PHYSICAL_PAGE | 7);
        let mut session = MutableTranslationProbeSession::new(
            &mut memory_first,
            config(0x8001_0033, ROOT, 0x20, 0xd00),
        )
        .unwrap();
        let _ = session
            .probe(linear, AccessKind::Read, explicit_ring3())
            .unwrap();
        let contradictory_retirement = PagedProbeResult {
            exit: PagedProbeExit::Retired,
            instructions_executed: 0,
            rflags_defined: od_isa::INTEGER_FLAGS,
        };
        assert!(matches!(
            session.consume_retired_cr0_result(&contradictory_retirement, &cpu),
            Err(MutableTranslationSessionError::Cr0RetiredWithoutEffect {
                instructions_executed: 0,
                ..
            })
        ));
        assert!(session.is_cache_empty());
        assert_eq!(
            session.probe(linear, AccessKind::Read, explicit_ring3()),
            Err(MutableTranslationSessionError::SessionPoisoned)
        );

        let mut memory = memory(PHYSICAL_PAGE | 7);
        let mut session = MutableTranslationProbeSession::new(
            &mut memory,
            config(0x8001_0033, ROOT, 0x20, 0xd00),
        )
        .unwrap();
        let _ = session
            .probe(linear, AccessKind::Read, explicit_ring3())
            .unwrap();
        let valid_nonretired = PagedProbeResult {
            exit: PagedProbeExit::Unsupported { opcode: 0x90 },
            instructions_executed: 0,
            rflags_defined: od_isa::INTEGER_FLAGS,
        };
        assert!(
            session
                .consume_retired_cr0_result(&valid_nonretired, &cpu)
                .is_ok()
        );
        assert!(!session.is_cache_empty());
        assert!(matches!(
            session.probe(linear, AccessKind::Read, explicit_ring3()),
            Ok(TranslationProbeCacheOutcome::Hit(_))
        ));
    }

    #[test]
    fn mov_cr3_fetch_does_not_write_ad_bits_or_touch_data_memory() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &[0x0f, 0x22, 0xd8],
                |_| {},
            )
            .unwrap();
        let table_addresses = [ROOT, PDPT, PD + 16, PT];
        let before = table_addresses
            .map(|address| memory.read_u64(GuestPhysicalAddress::new(address)).unwrap());
        let paging = config(0x8001_0033, ROOT, 0x20, 0xd00);
        let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
        let mut callbacks = 0;
        session
            .write_pte(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 7, |_| {
                callbacks += 1
            })
            .unwrap();
        assert_eq!(callbacks, 1);
        callbacks = 0;

        let mut cpu = mov_cr3_cpu(linear.get(), ROOT);
        let result = session
            .execute_mov_cr3(&mut cpu, mov_cr3_context(), od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(result.instructions_executed, 1, "only MOV CR3 retires");
        assert_eq!(callbacks, 0, "effect consumption has no write callback");
        assert!(session.is_cache_empty());
        assert!(
            matches!(
                session
                    .probe(linear, AccessKind::Read, explicit_ring3())
                    .unwrap(),
                TranslationProbeCacheOutcome::Walk(_)
            ),
            "only instruction fetch uses the pure walker before this lookup"
        );
        assert_eq!(
            table_addresses.map(|address| session
                .memory
                .read_u64(GuestPhysicalAddress::new(address))
                .unwrap()),
            before,
            "pure fetch and CR3 effect consumption must not commit A/D bits"
        );
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
    fn wp_invlpg_invalidates_stale_mapping_for_both_wp_states_with_pure_fetch() {
        for cr0 in [0x8001_0033, 0x8000_0033] {
            let mut memory = memory(PHYSICAL_PAGE | 7);
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                    &[0x0f, 0x01, 0x3b],
                    |_| {},
                )
                .unwrap();
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(0xa000 + 0x20),
                    &[0x0f, 0x01, 0x3b],
                    |_| {},
                )
                .unwrap();
            let paging = config(cr0, ROOT, 0x20, 0xd00);
            let ring0 = mov_cr3_context();
            let linear = GuestLinearAddress::new(LINEAR_PAGE + 0x70);
            let instruction = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
            let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();

            let (hit, old) = translated(session.probe(linear, AccessKind::Read, ring0).unwrap());
            assert!(!hit);
            assert_eq!(old.physical.get(), PHYSICAL_PAGE + 0x70);
            assert!(matches!(
                session.probe(linear, AccessKind::Read, ring0).unwrap(),
                TranslationProbeCacheOutcome::Hit(_)
            ));
            session
                .write_pte(GuestPhysicalAddress::new(PT), 0xa000 | 7, |_| {})
                .unwrap();
            let (hit, stale) = translated(session.probe(linear, AccessKind::Read, ring0).unwrap());
            assert!(
                hit,
                "PTE mutation alone must preserve the matching stale hit"
            );
            assert_eq!(stale.physical.get(), PHYSICAL_PAGE + 0x70);
            assert!(!session.is_cache_empty());

            let entries_before = [ROOT, PDPT, PD + 16, PT].map(|address| {
                session
                    .memory
                    .read_u64(GuestPhysicalAddress::new(address))
                    .unwrap()
            });
            let mut cpu = invlpg_cpu(instruction.get(), linear.get());
            cpu.cr0 = Some(cr0);
            let result = session
                .execute_invlpg_wp(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
                .unwrap();
            let entries_after = [ROOT, PDPT, PD + 16, PT].map(|address| {
                session
                    .memory
                    .read_u64(GuestPhysicalAddress::new(address))
                    .unwrap()
            });
            assert_eq!(
                entries_after, entries_before,
                "INVLPG fetch must not write A/D"
            );
            assert_eq!(
                result.exit,
                InvlpgExit::TranslationInvalidation(TranslationInvalidation::Invlpg {
                    linear_address: linear.get(),
                })
            );
            assert_eq!(result.instructions_executed, 1);
            assert!(
                session.is_cache_empty(),
                "canonical effect clears synchronously"
            );
            let (hit, updated) =
                translated(session.probe(linear, AccessKind::Read, ring0).unwrap());
            assert!(!hit, "first post-effect lookup must walk the changed PTE");
            assert_eq!(updated.physical.get(), 0xa070);
            assert!(matches!(
                session.probe(linear, AccessKind::Read, ring0).unwrap(),
                TranslationProbeCacheOutcome::Hit(_)
            ));
        }
    }

    #[test]
    fn wp_invlpg_noncanonical_nop_and_rejected_paths_preserve_cache() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        for (offset, bytes) in [
            (0x20, &[0x0f, 0x01, 0x3b][..]),
            (0x30, &[0xf0, 0x0f, 0x01, 0x3b][..]),
            (0x40, &[0x0f, 0x01, 0x3b][..]),
            (0x50, &[0x0f, 0x01, 0xfb][..]),
        ] {
            memory
                .write_bytes(
                    GuestPhysicalAddress::new(PHYSICAL_PAGE + offset),
                    bytes,
                    |_| {},
                )
                .unwrap();
        }
        let wp0 = 0x8000_0033;
        let paging = config(wp0, ROOT, 0x20, 0xd00);
        let ring0 = mov_cr3_context();
        let ring1 = AccessContext {
            privilege: PrivilegeLevel::Ring1,
            ..ring0
        };
        let key = GuestLinearAddress::new(LINEAR_PAGE + 0x10);
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let mut nop_cpu = invlpg_cpu(LINEAR_PAGE + 0x20, 0x0000_8000_0000_0000);
        nop_cpu.cr0 = Some(wp0);
        let nop = session
            .execute_invlpg_wp(&mut nop_cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(nop.exit, InvlpgExit::Retired);
        assert_eq!(nop.instructions_executed, 1);
        assert!(
            !session.is_cache_empty(),
            "noncanonical NOP keeps cache resident"
        );
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let before_mismatch = invlpg_cpu(LINEAR_PAGE + 0x20, LINEAR_PAGE + 0x80);
        let mut mismatch = before_mismatch.clone();
        assert_eq!(
            session.execute_invlpg_wp(&mut mismatch, ring0, od_isa::INTEGER_FLAGS),
            Err(MutableTranslationSessionError::UnsupportedPagingProfile)
        );
        assert_eq!(mismatch, before_mismatch);
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let mut cr3_mismatch = invlpg_cpu(LINEAR_PAGE + 0x20, LINEAR_PAGE + 0x80);
        cr3_mismatch.cr0 = Some(wp0);
        cr3_mismatch.cr3 = Some(ROOT + 0x1000);
        assert!(matches!(
            session.execute_invlpg_wp(&mut cr3_mismatch, ring0, od_isa::INTEGER_FLAGS),
            Err(MutableTranslationSessionError::Cr3Mismatch { .. })
        ));
        let mut cr4_mismatch = invlpg_cpu(LINEAR_PAGE + 0x20, LINEAR_PAGE + 0x80);
        cr4_mismatch.cr0 = Some(wp0);
        cr4_mismatch.cr4 = Some(0x30);
        assert_eq!(
            session.execute_invlpg_wp(&mut cr4_mismatch, ring0, od_isa::INTEGER_FLAGS),
            Err(MutableTranslationSessionError::UnsupportedPagingProfile)
        );
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        for (rip, context, expected_vector) in [
            (LINEAR_PAGE + 0x20, ring1, 13),
            (
                LINEAR_PAGE + 0x20,
                AccessContext {
                    privilege: PrivilegeLevel::Ring2,
                    ..ring0
                },
                13,
            ),
            (
                LINEAR_PAGE + 0x20,
                AccessContext {
                    privilege: PrivilegeLevel::Ring3,
                    ..ring0
                },
                13,
            ),
        ] {
            assert!(matches!(
                session.probe(key, AccessKind::Read, ring0).unwrap(),
                TranslationProbeCacheOutcome::Hit(_)
            ));
            let mut cpu = invlpg_cpu(rip, LINEAR_PAGE + 0x80);
            cpu.cr0 = Some(wp0);
            let before = cpu.clone();
            let result = session
                .execute_invlpg_wp(&mut cpu, context, od_isa::INTEGER_FLAGS)
                .unwrap();
            assert_eq!(
                result.exit,
                InvlpgExit::Fault(od_isa::memory::ArchitecturalFault {
                    vector: expected_vector,
                    error_code: Some(0),
                    linear_address: None,
                })
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(cpu, before);
            assert!(matches!(
                session.probe(key, AccessKind::Read, ring0).unwrap(),
                TranslationProbeCacheOutcome::Hit(_)
            ));
        }

        let mut lock_cpu = invlpg_cpu(LINEAR_PAGE + 0x30, LINEAR_PAGE + 0x80);
        lock_cpu.cr0 = Some(wp0);
        let lock_before = lock_cpu.clone();
        let lock = session
            .execute_invlpg_wp(&mut lock_cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(
            lock.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(lock.instructions_executed, 0);
        assert_eq!(lock_cpu, lock_before);
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        session
            .write_pte(GuestPhysicalAddress::new(PT), 0, |_| {})
            .unwrap();
        let mut fetch_fault_cpu = invlpg_cpu(LINEAR_PAGE + 0x40, LINEAR_PAGE + 0x80);
        fetch_fault_cpu.cr0 = Some(wp0);
        let before = fetch_fault_cpu.clone();
        let fetch_fault = session
            .execute_invlpg_wp(&mut fetch_fault_cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert!(matches!(
            fetch_fault.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(address),
            }) if address == LINEAR_PAGE + 0x40
        ));
        assert_eq!(fetch_fault.instructions_executed, 0);
        assert_eq!(fetch_fault_cpu, before);
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));

        let mut unsupported_cpu = invlpg_cpu(LINEAR_PAGE + 0x50, LINEAR_PAGE + 0x80);
        unsupported_cpu.cr0 = Some(wp0);
        session
            .write_pte(GuestPhysicalAddress::new(PT), PHYSICAL_PAGE | 7, |_| {})
            .unwrap();
        let unsupported = session
            .execute_invlpg_wp(&mut unsupported_cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(unsupported.exit, InvlpgExit::Unsupported { opcode: 0x0f });
        assert_eq!(unsupported.instructions_executed, 0);
        assert!(matches!(
            session.probe(key, AccessKind::Read, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(_)
        ));
    }

    #[test]
    fn wp_invlpg_fetch_fault_bypasses_matching_resident_fetch_translation() {
        let mut memory = memory(PHYSICAL_PAGE | 7);
        let rip = GuestLinearAddress::new(LINEAR_PAGE + 0x20);
        memory
            .write_bytes(
                GuestPhysicalAddress::new(PHYSICAL_PAGE + 0x20),
                &[0x0f, 0x01, 0x3b],
                |_| {},
            )
            .unwrap();
        let wp0 = 0x8000_0033;
        let paging = config(wp0, ROOT, 0x20, 0xd00);
        let ring0 = mov_cr3_context();
        let mut session = MutableTranslationProbeSession::new(&mut memory, paging).unwrap();

        assert!(matches!(
            session.probe(rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Walk(_)
        ));
        assert!(matches!(
            session.probe(rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(ref translation)
                if translation.physical.get() == PHYSICAL_PAGE + 0x20
        ));
        session
            .write_pte(GuestPhysicalAddress::new(PT), 0, |_| {})
            .unwrap();

        let mut cpu = invlpg_cpu(rip.get(), LINEAR_PAGE + 0x80);
        cpu.cr0 = Some(wp0);
        let before = cpu.clone();
        let result = session
            .execute_invlpg_wp(&mut cpu, ring0, od_isa::INTEGER_FLAGS)
            .unwrap();
        assert_eq!(
            result.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(rip.get()),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert!(matches!(
            session.probe(rip, AccessKind::Fetch, ring0).unwrap(),
            TranslationProbeCacheOutcome::Hit(ref translation)
                if translation.physical.get() == PHYSICAL_PAGE + 0x20
        ));
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
