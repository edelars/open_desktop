//! 32-bit paging walk of the board's RAM (S2-25 remainder, CR0.PG = 1, CR4.PAE = 0, EFER.LME = 0).
//!
//! The two-level walk of Intel SDM Vol. 3A (Rev. 093) section 5.3: bits 31:22 of the 32-bit linear
//! address select a 4-byte entry of the page directory (CR3 bits 31:12) and, unless the PDE has
//! PS = 1 with CR4.PSE = 1 (a 4 MiB page), bits 21:12 select a 4-byte entry of the page table. It is
//! the PhysicalMemory counterpart of od_isa::paging::translate32_with (S2-25, S2-25j) and has the
//! same rights model: with PSE = 0 no bit of a PDE or PTE is reserved; with PSE = 1 bits 21:13 of a
//! 4 MiB PDE are reserved (21:17 with the PSE-36 model switch, bits 16:13 then being physical-address
//! bits 35:32); there is no XD; a write to a page whose PDE or PTE has R/W = 0 faults at CPL 3 or with
//! CR0.WP = 1. What this walk adds on top of the od_isa model is what the PAE walk of this crate has
//! (see pae.rs): the user/supervisor check for CPL 3, the page-fault error code, the walk trace and
//! the accessed and dirty bits as one compare-and-replace (S6-25).
//!
//! The entries are 4 bytes, the unit of the compare-and-replace is the 8-byte cell the entry lies in
//! (PhysicalMemory::compare_replace_u64): the other dword of the cell is carried over, and a change
//! of that dword alone is not a change of the entry.
//!
//! Differences from the od_isa model, on purpose (as in the PAE walk):
//! - the physical address is not limited to 4 GiB: a PSE-36 4 MiB page above 4 GiB translates and the
//!   board's memory map decides what lies there (od_isa reports PhysicalBeyond4GiB);
//! - the accessed bit of an entry is set when the walk reaches it, also when a later entry or the
//!   access rights then fault; od_isa sets accessed and dirty bits only after a successful walk;
//! - the page-fault error code has no I/D bit (bit 4): CR4.SMEP is not allowed in the snapshot and
//!   there is no XD, so no fetch fault is an instruction-fetch fault in the sense of SDM section 5.7.
//!
//! Not here: CR4.SMEP (the snapshot refuses it, as the PAE one does), CR4.PGE and global pages (no
//! TLB), CR4.PKE. There is no hardware oracle for it (the Oracle machine's guests run in long mode);
//! it is checked against the od_isa model and by hand against the SDM.

use super::pae::{PaeOperandOperation, access_operand_with_walk};
use super::*;
use od_isa::protected_exec::ProtectedCpu;

#[cfg(test)]
mod operand_tests;
#[cfg(test)]
mod tests;

/// Bits 21:13 of a 4 MiB PDE with PSE = 1 and no PSE-36: reserved (SDM Vol. 3A Table 5-4).
const PDE_4M_RESERVED: u64 = 0x3F_E000;
/// Bits 21:17 of a 4 MiB PDE with the PSE-36 model switch: bits 21:M-19 with M = min(40, MAXPHYADDR) =
/// 36; bits 16:13 are physical-address bits 35:32.
const PDE_4M_RESERVED_PSE36: u64 = 0x3E_0000;
/// The page-table (or page) frame of a 4 KiB entry and the page-directory frame of CR3.
const FRAME_4K: u64 = 0xFFFF_F000;
/// The frame (address bits 31:22) of a 4 MiB PDE.
const FRAME_4M: u64 = 0xFFC0_0000;
/// Offset bits of a 4 MiB page.
const OFFSET_4M: u64 = 0x3F_FFFF;
/// EFER bits a 32-bit paging configuration may have: LME and LMA are clear; NXE is inert (no XD
/// without PAE) and kept as a snapshot-only bit, as CR4.PGE is.
const EFER_PG32_ALLOWED: u64 = EFER_SCE | EFER_NXE;

/// Explicit snapshot of 32-bit paging: the control registers and the PSE-36 model switch. The walk
/// reads the page directory at CR3 bits 31:12.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pg32ProbeConfig {
    cr0: u64,
    cr3: u64,
    cr4: u64,
    efer: u64,
    pse36: bool,
}

impl Pg32ProbeConfig {
    /// A snapshot of 32-bit paging. CR0.PE, CR0.PG and CR0.ET must be set, CR4.PAE, EFER.LME and
    /// EFER.LMA clear, CR3 a 32-bit value (its bits 11:0 are ignored by the walk). `pse36` is the model
    /// switch of the PSE-36 mechanism (CPUID.01H:EDX[17] is not declared in any profile; declaring
    /// it is S2-57): it matters only with CR4.PSE = 1.
    pub fn new(
        cr0: u64,
        cr3: u64,
        cr4: u64,
        efer: u64,
        pse36: bool,
    ) -> Result<Self, ProbeConfigError> {
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
        if cr4 & CR4_PAE != 0 {
            return Err(ProbeConfigError::UnsupportedBits {
                register: "CR4",
                value: cr4,
                offending_bits: CR4_PAE,
            });
        }
        validate_mask("EFER", efer, EFER_PG32_ALLOWED)?;
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
            pse36,
        })
    }

    /// The 32-bit paging of a legacy protected-mode model ([`ProtectedCpu`]): its control registers
    /// and its PSE-36 model switch. A model without 32-bit paging (CR0.PG clear or CR4.PAE set) is an
    /// error.
    pub fn from_protected_cpu(cpu: &ProtectedCpu) -> Result<Self, ProbeConfigError> {
        Self::new(
            u64::from(cpu.cr0),
            u64::from(cpu.cr3),
            u64::from(cpu.cr4),
            cpu.efer,
            cpu.pse36,
        )
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
    pub const fn pse36(self) -> bool {
        self.pse36
    }

    fn write_protect(self) -> bool {
        self.cr0 & CR0_WP != 0
    }
    /// CR4.PSE: a PDE with PS = 1 maps a 4 MiB page (with PSE = 0 the PS bit is ignored).
    fn pse(self) -> bool {
        self.cr4 & CR4_PSE != 0
    }
}

/// Size of the page a 32-bit paging walk ended in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pg32PageSize {
    Size4K,
    Size4M,
}

/// A finished 32-bit paging translation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pg32Translation {
    pub linear: u32,
    /// Up to 36 bits with the PSE-36 switch (a 4 MiB page), 32 bits otherwise.
    pub physical: GuestPhysicalAddress,
    pub page_size: Pg32PageSize,
    /// The PDE (4 MiB) or PTE (4 KiB) that mapped the page, and its 32-bit value with the accessed bit
    /// set. The committing forms write that bit into memory; the read-only forms
    /// ([`translate_pg32_ram_read_only`]) do not: memory keeps the old entry, only this returned
    /// value has A = 1.
    pub leaf_entry_address: GuestPhysicalAddress,
    pub leaf_entry_value: u64,
    /// Physical bases of the tables visited, indexed like the IA-32e walk (PML4, PDPT, PD, PT): the
    /// first two are always `None`, the PD is `CR3 & 0xFFFF_F000`, the PT is `None` past a 4 MiB page.
    pub table_pages: [Option<GuestPhysicalAddress>; 4],
    /// The accessed/dirty updates this translation committed (empty for the read-only forms).
    pub ad_updates: Vec<AdUpdate>,
}

fn pg32_translation(
    linear: u32,
    translation: &WalkTranslation,
    ad_updates: Vec<AdUpdate>,
) -> Pg32Translation {
    Pg32Translation {
        linear,
        physical: translation.physical,
        page_size: if translation.leaf_level == PageTableLevel::Pd {
            Pg32PageSize::Size4M
        } else {
            Pg32PageSize::Size4K
        },
        leaf_entry_address: translation.leaf_address,
        leaf_entry_value: translation.leaf_value,
        table_pages: translation.table_pages,
        ad_updates,
    }
}

/// One 4-byte entry of 32-bit paging at `entry_address` (4-aligned): the dword of the 8-byte cell it
/// lies in. Tables live in RAM only ([`read_table_entry`] refuses a framebuffer).
fn read_entry32(
    memory: &PhysicalMemory,
    entry_address: GuestPhysicalAddress,
) -> Result<u64, MemoryAccessError> {
    let cell_address = GuestPhysicalAddress::new(entry_address.get() & !7);
    let cell = read_table_entry(memory, cell_address)?;
    Ok(u64::from((cell >> ((entry_address.get() & 4) * 8)) as u32))
}

/// The table-entry writer of 32-bit paging (the seam of [`TrackedTableWriter`], values are the
/// 32-bit entries): a compare-and-replace of one dword, done as the compare-and-replace of the
/// 8-byte cell around it with the other dword carried over. When only the other dword changed
/// between the read and the replace the cell is read again; when the entry itself changed nothing is
/// written and its value is reported ([`TableReplace::Changed`]).
fn compare_replace_entry32(
    memory: &PhysicalMemory,
    address: GuestPhysicalAddress,
    expected: u64,
    replacement: u64,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
) -> Result<TableReplace, MemoryAccessError> {
    let cell_address = GuestPhysicalAddress::new(address.get() & !7);
    let shift = (address.get() & 4) * 8;
    let dword = 0xFFFF_FFFFu64 << shift;
    let mut observed = expected;
    for _ in 0..=TABLE_REPLACE_RETRY_LIMIT {
        let cell = memory.load_u64(cell_address)?;
        observed = (cell & dword) >> shift;
        if observed != expected {
            return Ok(TableReplace::Changed(observed));
        }
        let replaced = (cell & !dword) | ((replacement << shift) & dword);
        if memory
            .compare_replace_u64(cell_address, cell, replaced, &mut *committed_write)?
            .is_ok()
        {
            return Ok(TableReplace::Replaced);
        }
    }
    // The other dword kept changing: report the entry as unchanged-but-not-replaced; the caller's
    // retry bound ends the walk with TableEntryChanged.
    Ok(TableReplace::Changed(observed))
}

/// The 32-bit paging walk. The levels are reported as [`PageTableLevel::Pd`] and
/// [`PageTableLevel::Pt`]; the entry values of the trace and of the accessed/dirty updates are the
/// 32-bit entries. Past the table checks: `P = 0` is not present at any level; a 4 MiB PDE (PSE = 1,
/// PS = 1) with a reserved bit is a reserved-bit fault; the effective rights are the AND of R/W and of
/// U/S of the PDE and the PTE (of the PDE alone for a 4 MiB page).
#[allow(clippy::too_many_arguments)]
fn walk_pg32_with_writer(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
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

    let mut table_base = paging.cr3 & FRAME_4K;
    let mut effective = EffectivePermissions {
        user: true,
        writable: true,
        execute_disabled: false,
    };
    let mut leaf = (GuestPhysicalAddress::new(0), 0u64);
    let mut large_leaf = false;
    let mut table_pages = [None; 4];
    for (table_index, level, shift) in [
        (2usize, PageTableLevel::Pd, 22),
        (3, PageTableLevel::Pt, 12),
    ] {
        table_pages[table_index] = Some(GuestPhysicalAddress::new(table_base));
        let index = ((address >> shift) & 0x3ff) as u16;
        let entry_address = GuestPhysicalAddress::new(table_base + u64::from(index) * 4);
        // S6-25: the accessed bit is a compare-and-replace of the entry just read; when the
        // replace fails the entry changed under the walk and the walk repeats from this level.
        let mut failed_replaces = 0;
        let (entry_value, is_large_leaf) = loop {
            let entry_value = match read_entry32(memory, entry_address) {
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
                    page_fault_error_code(access, context, false, false, false),
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
            let is_large_leaf =
                level == PageTableLevel::Pd && paging.pse() && entry_value & ENTRY_PS_OR_PAT != 0;
            let offending = if is_large_leaf {
                entry_value
                    & if paging.pse36 {
                        PDE_4M_RESERVED_PSE36
                    } else {
                        PDE_4M_RESERVED
                    }
            } else {
                0
            };
            if offending != 0 {
                let record = PageFaultRecord::new(
                    instruction_rip,
                    access,
                    context,
                    page_fault_error_code(access, context, true, true, false),
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
        leaf = (entry_address, entry_value | (1 << 5));
        if is_large_leaf {
            large_leaf = true;
            break;
        }
        if level != PageTableLevel::Pt {
            table_base = entry_value & FRAME_4K;
        }
    }

    let user = context.page_privilege() == PrivilegeLevel::Ring3;
    let user_denied = user && !effective.user;
    let write_denied =
        access == AccessKind::Write && !effective.writable && (user || paging.write_protect());
    if user_denied || write_denied {
        let record = PageFaultRecord::new(
            instruction_rip,
            access,
            context,
            page_fault_error_code(access, context, true, false, false),
            address,
        );
        return fail(
            TrackedAccessError::PageFault {
                record,
                reason: PageFaultReason::Protection {
                    user_denied,
                    write_denied,
                    execute_denied: false,
                },
            },
            trace,
            ad_updates,
            ad_update_count,
        );
    }
    let physical = if large_leaf {
        // PSE-36: PDE bits 16:13 are physical-address bits 35:32 (bits 21:17 are reserved).
        let high = if paging.pse36 {
            (leaf.1 >> 13) & 0xF
        } else {
            0
        };
        (high << 32) | (leaf.1 & FRAME_4M) | (address & OFFSET_4M)
    } else {
        (leaf.1 & FRAME_4K) | (address & 0xfff)
    };
    WalkResult {
        translation: Ok(WalkTranslation {
            physical: GuestPhysicalAddress::new(physical),
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

/// Translates `linear` through 32-bit paging WITHOUT committing anything: the table writer is a
/// no-op, so no accessed or dirty bit changes in guest RAM (the board's fetch view and stop
/// diagnostics must not alter the tables they report on). `context` carries the CPL (and the origin
/// that decides the page privilege); its `mode` plays no part here.
pub fn translate_pg32_ram_read_only(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
) -> Result<Pg32Translation, TrackedAccessError> {
    translate_pg32_ram_read_only_with_trace(
        memory,
        paging,
        instruction_rip,
        linear,
        context,
        access,
    )
    .0
}

/// [`translate_pg32_ram_read_only`] with the PD and PT entries the walk observed, including those
/// visited before a fault.
pub fn translate_pg32_ram_read_only_with_trace(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
) -> (Result<Pg32Translation, TrackedAccessError>, WalkTrace) {
    let mut no_op_writer =
        |_: &PhysicalMemory,
         _: GuestPhysicalAddress,
         _: u64,
         _: u64,
         _: &mut dyn FnMut(GuestPhysicalRange)| Ok(TableReplace::Replaced);
    let walk = walk_pg32_with_writer(
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
            .map(|translation| pg32_translation(linear, &translation, Vec::new())),
        walk.trace,
    )
}

/// Translates `linear` through 32-bit paging as an access of kind `access` does: the accessed bit of
/// every entry the walk uses and, for a write, the dirty bit of the leaf are committed (each one
/// compare-and-replace of the entry, reported through `committed_write` and in
/// [`Pg32Translation::ad_updates`]). There is no operand access: the caller reads or writes the
/// physical address, and checks its region kind.
pub fn translate_pg32_ram(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
    instruction_rip: u64,
    linear: u32,
    context: AccessContext,
    access: AccessKind,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<Pg32Translation, TrackedAccessError> {
    let walk = walk_pg32_with_writer(
        memory,
        paging,
        instruction_rip,
        linear,
        context,
        access,
        &mut committed_write,
        &mut compare_replace_entry32,
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
            &mut compare_replace_entry32,
            &mut committed_write,
            translation.leaf_level,
            translation.leaf_address,
            translation.leaf_value,
        )?
    {
        updates.push(update);
    }
    Ok(pg32_translation(linear, &translation, updates))
}

/// A request for one operand access through 32-bit paging. `linear` is a 32-bit address (the operand
/// does not wrap past 4 GiB: it lies in one page); `context` carries the CPL. The operation is the
/// one of the PAE access ([`PaeOperandOperation`] names the access, not the paging mode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pg32AccessRequest {
    pub instruction_rip: u64,
    pub linear: u32,
    pub context: AccessContext,
    pub operation: PaeOperandOperation,
}

/// [`access_pg32_with_mmio`] without a device dispatch: an operand in an MMIO region is the typed
/// error of the RAM-only path of the 4-level walk (`DeviceUnavailable` for a write,
/// `OperandPhysical` with `UnreadableRegion(Mmio)` for a read), and no dirty bit is set for it.
pub fn access_pg32_ram(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
    request: Pg32AccessRequest,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> TrackedAccessReport {
    access_pg32_internal(memory, paging, request, &mut committed_write, None)
}

/// The operand access of 32-bit paging with the device dispatch of the physical address: the walk
/// (accessed bits of the PDE and PTE as the walk reaches them, the dirty bit of the leaf for a
/// write), then the region of the physical operand decides, exactly as for PAE paging
/// ([`super::pae::access_pae_with_mmio`]). A PSE-36 page above 4 GiB is looked up in the board's
/// memory map like any other physical address.
pub(crate) fn access_pg32_with_mmio(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
    request: Pg32AccessRequest,
    mut committed_write: impl FnMut(GuestPhysicalRange),
    mut mmio: impl FnMut(GuestPhysicalAddress, u8, RamAccessOperation) -> Result<u64, MmioDispatchError>,
) -> TrackedAccessReport {
    access_pg32_internal(
        memory,
        paging,
        request,
        &mut committed_write,
        Some(&mut mmio),
    )
}

fn access_pg32_internal(
    memory: &PhysicalMemory,
    paging: &Pg32ProbeConfig,
    request: Pg32AccessRequest,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
    mmio: Option<&mut MmioDispatch<'_>>,
) -> TrackedAccessReport {
    access_operand_with_walk(
        memory,
        request.linear,
        request.operation,
        committed_write,
        mmio,
        &mut compare_replace_entry32,
        |access, committed_write| {
            walk_pg32_with_writer(
                memory,
                paging,
                request.instruction_rip,
                request.linear,
                request.context,
                access,
                committed_write,
                &mut compare_replace_entry32,
            )
        },
    )
}
