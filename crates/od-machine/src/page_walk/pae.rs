//! PAE paging walk of the board's RAM (S2-25h, CR0.PG = 1, CR4.PAE = 1, EFER.LME = 0).
//!
//! The three-level walk of Intel SDM Vol. 3A (Rev. 093) section 5.4: the PDPTE register selected by
//! bits 31:30 of the 32-bit linear address, then the page directory and, unless the PDE has PS = 1
//! (a 2 MiB page), the page table. It is the `PhysicalMemory` counterpart of
//! `od_isa::paging::translate_pae` (S2-25) and has the same rights model: bits 62:36 of a PDE/PTE are
//! reserved when P = 1, XD is reserved while EFER.NXE = 0, bits 20:13 of a 2 MiB PDE are reserved, a
//! PDPTE has no R/W, U/S, XD or accessed bit and takes no part in the access rights. What this
//! walk adds on top of the `od_isa` model is the part the board's walk has for IA-32e paging: the
//! user/supervisor check for CPL 3, the page-fault error code, the walk trace and the accessed and
//! dirty bits as one compare-and-replace of the entry (S6-25).
//!
//! Differences from the `od_isa` model, on purpose:
//! - the physical address is not limited to 4 GiB (the board's memory map decides: 36 bits);
//! - the accessed bit of an entry is set when the walk reaches it, also when a later entry or the
//!   access rights then fault (the IA-32e walk of this crate does the same); `od_isa` sets accessed
//!   and dirty bits only after a successful walk;
//! - bits 52..=62 of an entry count as ignored when a concurrent writer changes only them between
//!   the walk and the dirty-bit update (`commit_leaf_dirty` is shared with the IA-32e walk); they
//!   are reserved in a PAE entry, so a guest that sets them gets #PF(RSVD) on its next walk.
//!
//! The long-mode walk (`walk_long64_4k_with_writer`) is not touched: this module is a separate walk
//! over the same helpers. There is no hardware oracle for it (the Oracle machine's guests run in
//! long mode); it is checked against the `od_isa` model and by hand against the SDM.

use super::*;
use od_isa::protected_exec::ProtectedCpu;

/// PDPTE bits that must be 0 when P = 1: 2:1, 8:5 and 63:MAXPHYADDR (SDM Vol. 3A Table 5-8).
const PDPTE_RESERVED: u64 = 0xFFFF_FFF0_0000_0000 | 0x1E6;
/// PDE/PTE bits 62:MAXPHYADDR with PAE paging, reserved when P = 1 (SDM Vol. 3A section 5.4.2).
const PAE_ENTRY_RESERVED: u64 = 0x7FFF_FFF0_0000_0000;
/// `EFER` bits a PAE paging configuration may have: LME and LMA are clear.
const EFER_PAE_ALLOWED: u64 = EFER_SCE | EFER_NXE;
/// `CR3` bits 4:0 are ignored with PAE paging (SDM Vol. 3A Table 5-7): the PDPT is 32-byte aligned.
const CR3_PDPT_MASK: u64 = 0xFFFF_FFE0;

/// Why a PAE paging configuration could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaeConfigError {
    /// A control register is not one of a PAE walk (the checks of [`Long64ProbeConfig`], with
    /// `EFER.LME = 0` and `EFER.LMA = 0` in place of 1, and a 32-bit `CR3`).
    Config(ProbeConfigError),
    /// A PDPTE with P = 1 has a reserved bit set: #GP(0) at the `MOV` to the control register that
    /// loads the PDPTEs (SDM Vol. 3A section 5.4.1). `index` is the PDPTE number.
    PdpteReserved { index: u8, pdpte: u64 },
    /// The 8 bytes of a PDPTE are not readable RAM.
    PdpteUnreadable {
        address: GuestPhysicalAddress,
        source: MemoryAccessError,
    },
}

impl fmt::Display for PaeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported PAE paging configuration: {self:?}")
    }
}
impl std::error::Error for PaeConfigError {}

/// Explicit snapshot of PAE paging: the control registers and the four PDPTE registers. The walk
/// does not read `CR3`; the PDPTEs were loaded at the `MOV` to `CR0`/`CR3`/`CR4` that enabled or
/// changed PAE paging ([`PaeProbeConfig::load`], or the `pdptes` of a [`ProtectedCpu`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaeProbeConfig {
    cr0: u64,
    cr3: u64,
    cr4: u64,
    efer: u64,
    pdptes: [u64; 4],
}

impl PaeProbeConfig {
    /// A snapshot of already loaded PDPTE registers. `CR0.PE`, `CR0.PG`, `CR0.ET` and `CR4.PAE` must
    /// be set, `EFER.LME` and `EFER.LMA` clear, `cr3` a 32-bit value; every PDPTE with P = 1 has its
    /// reserved bits clear.
    pub fn new(
        cr0: u64,
        cr3: u64,
        cr4: u64,
        efer: u64,
        pdptes: [u64; 4],
    ) -> Result<Self, PaeConfigError> {
        let config = Self::check_registers(cr0, cr3, cr4, efer).map_err(PaeConfigError::Config)?;
        for (index, &pdpte) in pdptes.iter().enumerate() {
            if pdpte & ENTRY_PRESENT != 0 && pdpte & PDPTE_RESERVED != 0 {
                return Err(PaeConfigError::PdpteReserved {
                    index: index as u8,
                    pdpte,
                });
            }
        }
        Ok(Self { pdptes, ..config })
    }

    /// Loads the PDPTEs from the table `cr3` names (what the processor does at the `MOV` that
    /// enables PAE paging or reloads `CR3`) and builds the snapshot; the refusals of
    /// [`load_pae_pdptes`] and [`Self::new`] apply.
    pub fn load(
        memory: &PhysicalMemory,
        cr0: u64,
        cr3: u64,
        cr4: u64,
        efer: u64,
    ) -> Result<Self, PaeConfigError> {
        Self::check_registers(cr0, cr3, cr4, efer).map_err(PaeConfigError::Config)?;
        let pdptes = load_pae_pdptes(memory, cr3)?;
        Self::new(cr0, cr3, cr4, efer, pdptes)
    }

    /// The PAE paging of a legacy protected-mode model ([`ProtectedCpu`]): its control registers
    /// and its PDPTE registers as they are. A model without PAE paging (`CR0.PG` or `CR4.PAE`
    /// clear, or in IA-32e mode) is a [`PaeConfigError::Config`].
    pub fn from_protected_cpu(cpu: &ProtectedCpu) -> Result<Self, PaeConfigError> {
        Self::new(
            u64::from(cpu.cr0),
            u64::from(cpu.cr3),
            u64::from(cpu.cr4),
            cpu.efer,
            cpu.pdptes,
        )
    }

    fn check_registers(cr0: u64, cr3: u64, cr4: u64, efer: u64) -> Result<Self, ProbeConfigError> {
        validate_mask("CR0", cr0, CR0_ALLOWED)?;
        if cr0 & CR0_ET == 0 {
            return Err(ProbeConfigError::Cr0EtRequired { value: cr0 });
        }
        if cr0 & (CR0_PE | CR0_PG) != CR0_PE | CR0_PG {
            return Err(ProbeConfigError::RequiredBits {
                register: "CR0",
                bits: CR0_PE | CR0_PG,
            });
        }
        validate_mask("CR4", cr4, CR4_ALLOWED)?;
        if cr4 & CR4_PAE == 0 {
            return Err(ProbeConfigError::RequiredBits {
                register: "CR4",
                bits: CR4_PAE,
            });
        }
        validate_mask("EFER", efer, EFER_PAE_ALLOWED)?;
        if cr3 >> 32 != 0 {
            return Err(ProbeConfigError::InvalidCr3 {
                value: cr3,
                offending_bits: cr3 & !0xFFFF_FFFF,
            });
        }
        Ok(Self {
            cr0,
            cr3,
            cr4,
            efer,
            pdptes: [0; 4],
        })
    }

    pub const fn cr0(self) -> u64 {
        self.cr0
    }
    pub const fn cr3(self) -> u64 {
        self.cr3
    }
    pub const fn cr4(self) -> u64 {
        self.cr4
    }
    pub const fn efer(self) -> u64 {
        self.efer
    }
    pub const fn pdptes(self) -> [u64; 4] {
        self.pdptes
    }

    fn write_protect(self) -> bool {
        self.cr0 & CR0_WP != 0
    }
    fn nxe(self) -> bool {
        self.efer & EFER_NXE != 0
    }
}

/// Loads the four PDPTEs from the 32-byte-aligned table at `cr3 & 0xFFFF_FFE0` (SDM Vol. 3A Table
/// 5-7: bits 4:0 of `CR3` are ignored). Reserved bits are checked only in a PDPTE with P = 1 (the SDM
/// notes that some processors check them also with P = 0; not modelled, as in `od_isa`). The table
/// must be RAM.
pub fn load_pae_pdptes(memory: &PhysicalMemory, cr3: u64) -> Result<[u64; 4], PaeConfigError> {
    let base = cr3 & CR3_PDPT_MASK;
    let mut pdptes = [0u64; 4];
    for (index, slot) in pdptes.iter_mut().enumerate() {
        let address = GuestPhysicalAddress::new(base + 8 * index as u64);
        let pdpte = read_table_entry(memory, address)
            .map_err(|source| PaeConfigError::PdpteUnreadable { address, source })?;
        if pdpte & ENTRY_PRESENT != 0 && pdpte & PDPTE_RESERVED != 0 {
            return Err(PaeConfigError::PdpteReserved {
                index: index as u8,
                pdpte,
            });
        }
        *slot = pdpte;
    }
    Ok(pdptes)
}

/// Size of the page a PAE walk ended in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaePageSize {
    Size4K,
    Size2M,
}

/// A finished PAE translation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaeTranslation {
    pub linear: u32,
    pub physical: GuestPhysicalAddress,
    pub page_size: PaePageSize,
    /// The PDE (2 MiB) or PTE (4 KiB) that mapped the page, and its value with the accessed bit
    /// set (as the walk leaves it).
    pub leaf_entry_address: GuestPhysicalAddress,
    pub leaf_entry_value: u64,
    /// Physical bases of the tables visited, indexed like the IA-32e walk (PML4, PDPT, PD, PT):
    /// the first two are always `None` (the PDPTE is a register, not a table in memory), the PT is
    /// `None` past a 2 MiB page.
    pub table_pages: [Option<GuestPhysicalAddress>; 4],
    /// The accessed/dirty updates this translation committed (empty for the read-only forms).
    pub ad_updates: Vec<AdUpdate>,
}

fn pae_translation(
    linear: u32,
    translation: &WalkTranslation,
    ad_updates: Vec<AdUpdate>,
) -> PaeTranslation {
    PaeTranslation {
        linear,
        physical: translation.physical,
        page_size: if translation.leaf_level == PageTableLevel::Pd {
            PaePageSize::Size2M
        } else {
            PaePageSize::Size4K
        },
        leaf_entry_address: translation.leaf_address,
        leaf_entry_value: translation.leaf_value,
        table_pages: translation.table_pages,
        ad_updates,
    }
}

/// The PAE walk. The PDPTE is a register: it appears neither in the trace (entries are PD and PT)
/// nor in the accessed-bit updates; a PDPTE with P = 0 is `PageFaultReason::NotPresent` at
/// `PageTableLevel::Pdpt` with no table read at all. The rest is [`walk_long64_4k_with_writer`] for
/// the two lower levels, with 2 MiB pages always admitted.
fn walk_pae_with_writer(
    memory: &PhysicalMemory,
    paging: &PaeProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
    table_writer: &mut TrackedTableWriter<'_>,
) -> WalkResult {
    let mut trace = WalkTrace::new();
    let mut ad_updates = [None; 4];
    let mut ad_update_count = 0;
    let address = u64::from(linear);
    let fail = |error, trace, ad_updates, ad_update_count| WalkResult {
        translation: Err(error),
        trace,
        ad_updates,
        ad_update_count,
    };

    let pdpte = paging.pdptes[(linear >> 30) as usize];
    if pdpte & ENTRY_PRESENT == 0 {
        let record = PageFaultRecord::new(
            instruction_rip,
            access,
            context,
            page_fault_error_code(access, context, false, false, paging.nxe()),
            address,
        );
        return fail(
            TrackedAccessError::PageFault {
                record,
                reason: PageFaultReason::NotPresent {
                    level: PageTableLevel::Pdpt,
                },
            },
            trace,
            ad_updates,
            ad_update_count,
        );
    }

    let mut table_base = pdpte & ENTRY_ADDRESS_MASK;
    let mut effective = EffectivePermissions {
        user: true,
        writable: true,
        execute_disabled: false,
    };
    let mut leaf = (GuestPhysicalAddress::new(0), 0u64);
    let mut large_leaf = false;
    let mut table_pages = [None; 4];
    for (table_index, level) in [(2usize, PageTableLevel::Pd), (3, PageTableLevel::Pt)] {
        table_pages[table_index] = Some(GuestPhysicalAddress::new(table_base));
        let index = ((address >> level.index_shift()) & 0x1ff) as u16;
        let entry_address = GuestPhysicalAddress::new(table_base + u64::from(index) * 8);
        // S6-25: the accessed bit is a compare-and-replace of the entry just read; when the
        // replace fails the entry changed under the walk and the walk repeats from this level.
        let mut failed_replaces = 0;
        let (entry_value, is_large_leaf) = loop {
            let entry_value = match read_table_entry(memory, entry_address) {
                Ok(value) => value,
                Err(source) => {
                    return fail(
                        TrackedAccessError::TableRead {
                            level,
                            entry_address,
                            source,
                        },
                        trace,
                        ad_updates,
                        ad_update_count,
                    );
                }
            };
            let walked = WalkEntry {
                level,
                index,
                entry_address,
                entry_value,
            };
            if failed_replaces == 0 {
                trace.push(walked);
            } else {
                trace.replace_last(walked);
            }
            if entry_value & ENTRY_PRESENT == 0 {
                let record = PageFaultRecord::new(
                    instruction_rip,
                    access,
                    context,
                    page_fault_error_code(access, context, false, false, paging.nxe()),
                    address,
                );
                return fail(
                    TrackedAccessError::PageFault {
                        record,
                        reason: PageFaultReason::NotPresent { level },
                    },
                    trace,
                    ad_updates,
                    ad_update_count,
                );
            }
            let mut offending = entry_value & PAE_ENTRY_RESERVED;
            if !paging.nxe() {
                offending |= entry_value & ENTRY_NX;
            }
            let is_large_leaf = level == PageTableLevel::Pd && entry_value & ENTRY_PS_OR_PAT != 0;
            if is_large_leaf {
                // Bits 20:13 of a 2 MiB PDE are reserved (address bits below 2 MiB).
                offending |= entry_value & ENTRY_2M_RESERVED;
            }
            if offending != 0 {
                let record = PageFaultRecord::new(
                    instruction_rip,
                    access,
                    context,
                    page_fault_error_code(access, context, true, true, paging.nxe()),
                    address,
                );
                return fail(
                    TrackedAccessError::PageFault {
                        record,
                        reason: PageFaultReason::ReservedBits {
                            level,
                            offending_bits: offending,
                        },
                    },
                    trace,
                    ad_updates,
                    ad_update_count,
                );
            }

            if entry_value & (1 << 5) == 0 {
                let after = entry_value | (1 << 5);
                match table_writer(
                    memory,
                    entry_address,
                    entry_value,
                    after,
                    &mut *committed_write,
                ) {
                    Ok(TableReplace::Replaced) => {
                        ad_updates[ad_update_count] = Some(AdUpdate {
                            level,
                            address: entry_address,
                            before: entry_value,
                            after,
                            bit: AdBit::Accessed,
                        });
                        ad_update_count += 1;
                    }
                    Ok(TableReplace::Changed(observed)) => {
                        failed_replaces += 1;
                        if failed_replaces > TABLE_REPLACE_RETRY_LIMIT {
                            return fail(
                                TrackedAccessError::TableEntryChanged {
                                    level,
                                    entry_address,
                                    bit: AdBit::Accessed,
                                    observed,
                                },
                                trace,
                                ad_updates,
                                ad_update_count,
                            );
                        }
                        continue;
                    }
                    Err(source) => {
                        return fail(
                            TrackedAccessError::TableWrite {
                                level,
                                entry_address,
                                bit: AdBit::Accessed,
                                source,
                            },
                            trace,
                            ad_updates,
                            ad_update_count,
                        );
                    }
                }
            }
            break (entry_value, is_large_leaf);
        };
        effective.user &= entry_value & ENTRY_USER != 0;
        effective.writable &= entry_value & ENTRY_WRITABLE != 0;
        effective.execute_disabled |= entry_value & ENTRY_NX_BIT != 0;
        leaf = (entry_address, entry_value | (1 << 5));
        if is_large_leaf {
            large_leaf = true;
            break;
        }
        if level != PageTableLevel::Pt {
            table_base = entry_value & ENTRY_ADDRESS_MASK;
        }
    }

    let user = context.page_privilege() == PrivilegeLevel::Ring3;
    let user_denied = user && !effective.user;
    let write_denied =
        access == AccessKind::Write && !effective.writable && (user || paging.write_protect());
    let execute_denied = access == AccessKind::Fetch && paging.nxe() && effective.execute_disabled;
    if user_denied || write_denied || execute_denied {
        let record = PageFaultRecord::new(
            instruction_rip,
            access,
            context,
            page_fault_error_code(access, context, true, false, paging.nxe()),
            address,
        );
        return fail(
            TrackedAccessError::PageFault {
                record,
                reason: PageFaultReason::Protection {
                    user_denied,
                    write_denied,
                    execute_denied,
                },
            },
            trace,
            ad_updates,
            ad_update_count,
        );
    }
    let offset_mask = if large_leaf { 0x1f_ffff } else { 0xfff };
    let physical = GuestPhysicalAddress::new(
        (leaf.1 & ENTRY_ADDRESS_MASK & !offset_mask) | (address & offset_mask),
    );
    WalkResult {
        translation: Ok(WalkTranslation {
            physical,
            leaf_address: leaf.0,
            leaf_value: leaf.1,
            leaf_level: if large_leaf {
                PageTableLevel::Pd
            } else {
                PageTableLevel::Pt
            },
            table_pages,
        }),
        trace,
        ad_updates,
        ad_update_count,
    }
}

/// Translates `linear` through PAE paging WITHOUT committing anything: the table writer is a no-op,
/// so no accessed or dirty bit changes in guest RAM (the board's fetch view and stop diagnostics must
/// not alter the tables they report on). `context` carries the CPL (and the origin that decides
/// the page privilege); its `mode` is the only value the type has and plays no part here.
pub fn translate_pae_ram_read_only(
    memory: &PhysicalMemory,
    paging: &PaeProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
) -> Result<PaeTranslation, TrackedAccessError> {
    translate_pae_ram_read_only_with_trace(memory, paging, instruction_rip, linear, context, access)
        .0
}

/// [`translate_pae_ram_read_only`] with the PD and PT entries the walk observed, including those
/// visited before a fault.
pub fn translate_pae_ram_read_only_with_trace(
    memory: &PhysicalMemory,
    paging: &PaeProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
) -> (Result<PaeTranslation, TrackedAccessError>, WalkTrace) {
    let mut no_op_writer =
        |_: &PhysicalMemory,
         _: GuestPhysicalAddress,
         _: u64,
         _: u64,
         _: &mut dyn FnMut(GuestPhysicalRange)| Ok(TableReplace::Replaced);
    let walk = walk_pae_with_writer(
        memory,
        paging,
        instruction_rip,
        linear,
        context,
        access,
        &mut |_| {},
        &mut no_op_writer,
    );
    (
        walk.translation
            .map(|translation| pae_translation(linear, &translation, Vec::new())),
        walk.trace,
    )
}

/// Translates `linear` through PAE paging as an access of kind `access` does: the accessed bit of
/// every entry the walk uses and, for a write, the dirty bit of the leaf are committed (each one
/// compare-and-replace of the entry, reported through `committed_write` and in
/// [`PaeTranslation::ad_updates`]). There is no operand access: the caller reads or writes the
/// physical address, and checks its region kind.
pub fn translate_pae_ram(
    memory: &PhysicalMemory,
    paging: &PaeProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PaeTranslation, TrackedAccessError> {
    let walk = walk_pae_with_writer(
        memory,
        paging,
        instruction_rip,
        linear,
        context,
        access,
        &mut committed_write,
        &mut compare_replace_table_entry,
    );
    let translation = walk.translation?;
    let mut updates: Vec<AdUpdate> = walk.ad_updates[..walk.ad_update_count]
        .iter()
        .flatten()
        .copied()
        .collect();
    if access == AccessKind::Write
        && let Some(update) = commit_leaf_dirty(
            memory,
            &mut compare_replace_table_entry,
            &mut committed_write,
            translation.leaf_level,
            translation.leaf_address,
            translation.leaf_value,
        )?
    {
        updates.push(update);
    }
    Ok(pae_translation(linear, &translation, updates))
}
