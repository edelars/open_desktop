//! Read-only diagnostic translation probe for the admitted Long64 4 KiB profile.

use crate::{GuestPhysicalAddress, MemoryAccessError, PhysicalMemory};
use od_isa::memory::{AccessContext, AccessKind, PrivilegeLevel};
use std::fmt;

const CR0_PE: u64 = 1 << 0;
const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_ET: u64 = 1 << 4;
const CR0_NE: u64 = 1 << 5;
const CR0_WP: u64 = 1 << 16;
const CR0_PG: u64 = 1 << 31;
const CR0_ALLOWED: u64 = CR0_PE | CR0_MP | CR0_EM | CR0_TS | CR0_ET | CR0_NE | CR0_WP | CR0_PG;
const CR4_PAE: u64 = 1 << 5;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_ALLOWED: u64 = CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT;
const EFER_SCE: u64 = 1;
const EFER_LME: u64 = 1 << 8;
const EFER_LMA: u64 = 1 << 10;
const EFER_NXE: u64 = 1 << 11;
const EFER_ALLOWED: u64 = EFER_SCE | EFER_LME | EFER_LMA | EFER_NXE;
const CR3_HIGH_MASK: u64 = !((1u64 << 36) - 1);
const ENTRY_ADDRESS_RESERVED: u64 = ((1u64 << 16) - 1) << 36;
const ENTRY_NX: u64 = 1 << 63;
const ENTRY_PRESENT: u64 = 1;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_PS_OR_PAT: u64 = 1 << 7;
const ENTRY_NX_BIT: u64 = 1 << 63;
const ENTRY_ADDRESS_MASK: u64 = ((1u64 << 36) - 1) & !0xfff;

/// Guest linear address; distinct from a guest physical address and host pointer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GuestLinearAddress(u64);

impl GuestLinearAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Explicit snapshot admitted by the bounded four-level Long64 probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Long64ProbeConfig {
    cr0: u64,
    cr3: u64,
    cr4: u64,
    efer: u64,
}

impl Long64ProbeConfig {
    pub fn new(cr0: u64, cr3: u64, cr4: u64, efer: u64) -> Result<Self, ProbeConfigError> {
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
        validate_mask("EFER", efer, EFER_ALLOWED)?;
        if efer & (EFER_LME | EFER_LMA) != EFER_LME | EFER_LMA {
            return Err(ProbeConfigError::RequiredBits {
                register: "EFER",
                bits: EFER_LME | EFER_LMA,
            });
        }
        if cr3 & CR3_HIGH_MASK != 0 {
            return Err(ProbeConfigError::InvalidCr3 {
                value: cr3,
                offending_bits: cr3 & CR3_HIGH_MASK,
            });
        }
        Ok(Self {
            cr0,
            cr3,
            cr4,
            efer,
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

    fn root(self) -> u64 {
        self.cr3 & ENTRY_ADDRESS_MASK
    }
    fn write_protect(self) -> bool {
        self.cr0 & CR0_WP != 0
    }
    fn nxe(self) -> bool {
        self.efer & EFER_NXE != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeConfigError {
    Cr0EtRequired {
        value: u64,
    },
    UnsupportedBits {
        register: &'static str,
        value: u64,
        offending_bits: u64,
    },
    RequiredBits {
        register: &'static str,
        bits: u64,
    },
    InvalidCr3 {
        value: u64,
        offending_bits: u64,
    },
}

impl fmt::Display for ProbeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported Long64 probe configuration: {self:?}")
    }
}
impl std::error::Error for ProbeConfigError {}

fn validate_mask(register: &'static str, value: u64, allowed: u64) -> Result<(), ProbeConfigError> {
    let offending_bits = value & !allowed;
    if offending_bits != 0 {
        return Err(ProbeConfigError::UnsupportedBits {
            register,
            value,
            offending_bits,
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageTableLevel {
    Pml4,
    Pdpt,
    Pd,
    Pt,
}

impl PageTableLevel {
    const fn index_shift(self) -> u32 {
        match self {
            Self::Pml4 => 39,
            Self::Pdpt => 30,
            Self::Pd => 21,
            Self::Pt => 12,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WalkEntry {
    pub level: PageTableLevel,
    pub index: u16,
    pub entry_address: GuestPhysicalAddress,
    pub entry_value: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WalkTrace {
    entries: [Option<WalkEntry>; 4],
    len: usize,
}

impl WalkTrace {
    const fn new() -> Self {
        Self {
            entries: [None; 4],
            len: 0,
        }
    }
    fn push(&mut self, entry: WalkEntry) {
        self.entries[self.len] = Some(entry);
        self.len += 1;
    }
    pub fn entries(&self) -> &[Option<WalkEntry>; 4] {
        &self.entries
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectivePermissions {
    pub user: bool,
    pub writable: bool,
    pub execute_disabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Translation4K {
    pub linear: GuestLinearAddress,
    pub physical: GuestPhysicalAddress,
    pub page_base: GuestPhysicalAddress,
    pub offset: u16,
    pub permissions: EffectivePermissions,
    pub leaf_entry_address: GuestPhysicalAddress,
    pub leaf_entry_value: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageFaultReason {
    NotPresent {
        level: PageTableLevel,
    },
    ReservedBits {
        level: PageTableLevel,
        offending_bits: u64,
    },
    Protection {
        user_denied: bool,
        write_denied: bool,
        execute_denied: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageFaultProbe {
    pub linear: GuestLinearAddress,
    pub access: AccessKind,
    pub page_privilege: PrivilegeLevel,
    /// Literal x86 PFEC fields for this probe result; this value is diagnostic only.
    pub error_code: u32,
    pub reason: PageFaultReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonCanonicalAddress {
    pub linear: GuestLinearAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeAccessError {
    TableRead {
        level: PageTableLevel,
        entry_address: GuestPhysicalAddress,
        source: MemoryAccessError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeError {
    NonCanonical(NonCanonicalAddress),
    UnsupportedPageSize {
        level: PageTableLevel,
        entry_address: GuestPhysicalAddress,
        entry_value: u64,
    },
    Access(ProbeAccessError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    Translated(Translation4K),
    PageFault(PageFaultProbe),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeReport {
    pub outcome: Result<ProbeOutcome, ProbeError>,
    pub trace: WalkTrace,
}

/// Walk exactly four little-endian entries. This pure probe has no TLB, no A/D
/// writes, no final operand access, and does not deliver architectural faults.
pub fn probe_long64_4k(
    memory: &PhysicalMemory,
    paging: &Long64ProbeConfig,
    address: GuestLinearAddress,
    access: AccessKind,
    context: AccessContext,
) -> ProbeReport {
    let mut trace = WalkTrace::new();
    if !is_canonical_48(address.0) {
        return ProbeReport {
            outcome: Err(ProbeError::NonCanonical(NonCanonicalAddress {
                linear: address,
            })),
            trace,
        };
    }
    let privilege = context.page_privilege();
    let mut table_base = paging.root();
    let levels = [
        PageTableLevel::Pml4,
        PageTableLevel::Pdpt,
        PageTableLevel::Pd,
        PageTableLevel::Pt,
    ];
    let mut effective = EffectivePermissions {
        user: true,
        writable: true,
        execute_disabled: false,
    };
    let mut leaf = (GuestPhysicalAddress::new(0), 0);
    for level in levels {
        let index = ((address.0 >> level.index_shift()) & 0x1ff) as u16;
        let entry_address = GuestPhysicalAddress::new(table_base + u64::from(index) * 8);
        let entry_value = match memory.read_u64(entry_address) {
            Ok(value) => value,
            Err(source) => {
                return ProbeReport {
                    outcome: Err(ProbeError::Access(ProbeAccessError::TableRead {
                        level,
                        entry_address,
                        source,
                    })),
                    trace,
                };
            }
        };
        trace.push(WalkEntry {
            level,
            index,
            entry_address,
            entry_value,
        });
        if entry_value & ENTRY_PRESENT == 0 {
            let fault = page_fault(
                address,
                access,
                privilege,
                false,
                false,
                level,
                PageFaultReason::NotPresent { level },
                paging.nxe(),
            );
            return ProbeReport {
                outcome: Ok(ProbeOutcome::PageFault(fault)),
                trace,
            };
        }
        let mut offending = entry_value & ENTRY_ADDRESS_RESERVED;
        if !paging.nxe() {
            offending |= entry_value & ENTRY_NX;
        }
        if level == PageTableLevel::Pml4 && entry_value & ENTRY_PS_OR_PAT != 0 {
            offending |= ENTRY_PS_OR_PAT;
        }
        if level != PageTableLevel::Pt
            && level != PageTableLevel::Pml4
            && entry_value & ENTRY_PS_OR_PAT != 0
        {
            return ProbeReport {
                outcome: Err(ProbeError::UnsupportedPageSize {
                    level,
                    entry_address,
                    entry_value,
                }),
                trace,
            };
        }
        if offending != 0 {
            let fault = page_fault(
                address,
                access,
                privilege,
                true,
                true,
                level,
                PageFaultReason::ReservedBits {
                    level,
                    offending_bits: offending,
                },
                paging.nxe(),
            );
            return ProbeReport {
                outcome: Ok(ProbeOutcome::PageFault(fault)),
                trace,
            };
        }
        effective.user &= entry_value & ENTRY_USER != 0;
        effective.writable &= entry_value & ENTRY_WRITABLE != 0;
        effective.execute_disabled |= entry_value & ENTRY_NX_BIT != 0;
        leaf = (entry_address, entry_value);
        if level != PageTableLevel::Pt {
            table_base = entry_value & ENTRY_ADDRESS_MASK;
        }
    }
    let is_user = privilege == PrivilegeLevel::Ring3;
    let user_denied = is_user && !effective.user;
    let write_denied =
        access == AccessKind::Write && !effective.writable && (is_user || paging.write_protect());
    let execute_denied = access == AccessKind::Fetch && paging.nxe() && effective.execute_disabled;
    if user_denied || write_denied || execute_denied {
        let fault = page_fault(
            address,
            access,
            privilege,
            true,
            false,
            PageTableLevel::Pt,
            PageFaultReason::Protection {
                user_denied,
                write_denied,
                execute_denied,
            },
            paging.nxe(),
        );
        return ProbeReport {
            outcome: Ok(ProbeOutcome::PageFault(fault)),
            trace,
        };
    }
    let offset = (address.0 & 0xfff) as u16;
    let page_base = GuestPhysicalAddress::new(leaf.1 & ENTRY_ADDRESS_MASK);
    let translation = Translation4K {
        linear: address,
        physical: GuestPhysicalAddress::new(page_base.get() | u64::from(offset)),
        page_base,
        offset,
        permissions: effective,
        leaf_entry_address: leaf.0,
        leaf_entry_value: leaf.1,
    };
    ProbeReport {
        outcome: Ok(ProbeOutcome::Translated(translation)),
        trace,
    }
}

fn page_fault(
    address: GuestLinearAddress,
    access: AccessKind,
    privilege: PrivilegeLevel,
    present: bool,
    reserved: bool,
    _level: PageTableLevel,
    reason: PageFaultReason,
    nxe: bool,
) -> PageFaultProbe {
    let mut error_code = u32::from(present)
        | (u32::from(access == AccessKind::Write) << 1)
        | (u32::from(privilege == PrivilegeLevel::Ring3) << 2)
        | (u32::from(reserved) << 3)
        | (u32::from(access == AccessKind::Fetch && nxe) << 4);
    if matches!(reason, PageFaultReason::NotPresent { .. }) {
        error_code &= !1;
    }
    PageFaultProbe {
        linear: address,
        access,
        page_privilege: privilege,
        error_code,
        reason,
    }
}

fn is_canonical_48(address: u64) -> bool {
    let sign = (address >> 47) & 1;
    let upper = address >> 48;
    if sign == 0 {
        upper == 0
    } else {
        upper == 0xffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GuestPhysicalRange, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, RamLimits,
    };

    const ROOT: u64 = 0x1000;
    const LINEAR: u64 = 0x0000_0080_8060_4abc;

    fn config(cr0: u64, cr3: u64, cr4: u64, efer: u64) -> Long64ProbeConfig {
        Long64ProbeConfig::new(cr0, cr3, cr4, efer).unwrap()
    }
    fn memory(size: u64) -> PhysicalMemory {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), size).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        PhysicalMemory::new(&map, RamLimits::new(size)).unwrap()
    }
    fn tables(mut mem: PhysicalMemory, entry: u64) -> PhysicalMemory {
        // Literal table addresses and indices are intentionally independent of production index helpers.
        for (addr, value) in [
            (0x1008, 0x2007),
            (0x2010, 0x3007),
            (0x3018, 0x4007),
            (0x4020, entry),
        ] {
            mem.write_u64(GuestPhysicalAddress::new(addr), value, |_| {})
                .unwrap();
        }
        mem
    }
    fn context(privilege: PrivilegeLevel) -> AccessContext {
        AccessContext {
            mode: od_isa::memory::InstructionMode::Long64,
            privilege,
            origin: od_isa::memory::AccessOrigin::Explicit,
        }
    }
    fn result(report: ProbeReport) -> ProbeOutcome {
        report.outcome.unwrap()
    }
    fn pf(report: ProbeReport) -> PageFaultProbe {
        match result(report) {
            ProbeOutcome::PageFault(pf) => pf,
            other => panic!("expected probe page fault, got {other:?}"),
        }
    }
    fn cfg(wp: bool, nxe: bool) -> Long64ProbeConfig {
        config(
            0x8000_0033 | if wp { CR0_WP } else { 0 },
            ROOT,
            CR4_PAE,
            EFER_LME | EFER_LMA | if nxe { EFER_NXE } else { 0 },
        )
    }

    #[test]
    fn independent_literal_table_geometry_and_high_canonical_half() {
        let mem = tables(memory(0x5000), 0x0000_000a_bcde_f007);
        let translated = match result(probe_long64_4k(
            &mem,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        )) {
            ProbeOutcome::Translated(t) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(translated.physical.get(), 0x0000_000a_bcde_fabc);
        assert_eq!(translated.page_base.get(), 0x0000_000a_bcde_f000);
        assert_eq!(translated.offset, 0xabc);
        assert_eq!(
            translated.permissions,
            EffectivePermissions {
                user: true,
                writable: true,
                execute_disabled: false
            }
        );
        assert_eq!(translated.leaf_entry_address.get(), 0x4020);
        assert_eq!(translated.leaf_entry_value, 0x0000_000a_bcde_f007);
        assert_eq!(
            translated.physical.get() & ((1 << 36) - 1),
            translated.physical.get()
        );
        // Canonical high-half linear address, with all four levels independently indexed at zero.
        let mut high_mem = memory(0x5000);
        for (addr, value) in [
            (0x1800, 0x2007),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4000, 0x8007),
        ] {
            high_mem
                .write_u64(GuestPhysicalAddress::new(addr), value, |_| {})
                .unwrap();
        }
        let high = result(probe_long64_4k(
            &high_mem,
            &cfg(false, false),
            GuestLinearAddress::new(0xffff_8000_0000_0000),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        assert!(matches!(high, ProbeOutcome::Translated(t) if t.physical.get() == 0x8000));
    }

    #[test]
    fn noncanonical_addresses_do_not_read_page_tables() {
        let mem = memory(0x5000);
        for address in [0x0000_8000_0000_0000, 0xffff_7fff_ffff_ffff] {
            let report = probe_long64_4k(
                &mem,
                &cfg(false, false),
                GuestLinearAddress::new(address),
                AccessKind::Read,
                context(PrivilegeLevel::Ring0),
            );
            assert!(matches!(report.outcome, Err(ProbeError::NonCanonical(_))));
            assert!(report.trace.is_empty());
        }
    }

    #[test]
    fn nonpresent_entries_at_each_level_report_literal_error_code() {
        for (address, level) in [
            (0x1008, PageTableLevel::Pml4),
            (0x2010, PageTableLevel::Pdpt),
            (0x3018, PageTableLevel::Pd),
            (0x4020, PageTableLevel::Pt),
        ] {
            let mut mem = tables(memory(0x5000), 0x4007);
            mem.write_u64(GuestPhysicalAddress::new(address), 0, |_| {})
                .unwrap();
            let report = probe_long64_4k(
                &mem,
                &cfg(false, false),
                GuestLinearAddress::new(LINEAR),
                AccessKind::Write,
                context(PrivilegeLevel::Ring3),
            );
            let fault = pf(report);
            assert_eq!(fault.reason, PageFaultReason::NotPresent { level });
            assert_eq!(fault.error_code, 0x06);
        }
    }

    #[test]
    fn reserved_nx_and_large_page_entries_have_distinct_results() {
        let mem = tables(memory(0x5000), 0x4007 | (1 << 36));
        let fault = pf(probe_long64_4k(
            &mem,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        assert_eq!(fault.error_code, 0x09);
        assert!(
            matches!(fault.reason, PageFaultReason::ReservedBits { level: PageTableLevel::Pt, offending_bits, .. } if offending_bits == 1 << 36)
        );
        let mem = tables(memory(0x5000), 0x4007 | ENTRY_NX);
        let fault = pf(probe_long64_4k(
            &mem,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        assert_eq!(fault.error_code, 0x09);
        let mut mem = tables(memory(0x5000), 0x4007);
        mem.write_u64(GuestPhysicalAddress::new(0x4020), 0, |_| {})
            .unwrap();
        let fault = pf(probe_long64_4k(
            &mem,
            &cfg(false, true),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Fetch,
            context(PrivilegeLevel::Ring3),
        ));
        assert_eq!(fault.error_code, 0x14);
        let mut mem = tables(memory(0x5000), 0x4007);
        mem.write_u64(
            GuestPhysicalAddress::new(0x3018),
            0x4007 | ENTRY_PS_OR_PAT,
            |_| {},
        )
        .unwrap();
        let report = probe_long64_4k(
            &mem,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        );
        assert!(matches!(
            report.outcome,
            Err(ProbeError::UnsupportedPageSize {
                level: PageTableLevel::Pd,
                ..
            })
        ));
    }

    #[test]
    fn permission_faults_cover_user_supervisor_write_fetch_and_wp() {
        let user_tables = tables(memory(0x5000), 0x4007);
        let user = context(PrivilegeLevel::Ring3);
        assert!(matches!(
            result(probe_long64_4k(
                &user_tables,
                &cfg(false, false),
                GuestLinearAddress::new(LINEAR),
                AccessKind::Read,
                user
            )),
            ProbeOutcome::Translated(_)
        ));
        for access in [AccessKind::Write, AccessKind::Fetch] {
            assert!(matches!(
                result(probe_long64_4k(
                    &user_tables,
                    &cfg(false, false),
                    GuestLinearAddress::new(LINEAR),
                    access,
                    user
                )),
                ProbeOutcome::Translated(_)
            ));
        }
        let supervisor_tables = tables(memory(0x5000), 0x4003);
        let fault = pf(probe_long64_4k(
            &supervisor_tables,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            user,
        ));
        assert_eq!(fault.error_code, 0x05);
        let fault = pf(probe_long64_4k(
            &supervisor_tables,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Write,
            user,
        ));
        assert_eq!(fault.error_code, 0x07);
        let supervisor = context(PrivilegeLevel::Ring0);
        let read_only = tables(memory(0x5000), 0x4005);
        assert!(matches!(
            result(probe_long64_4k(
                &read_only,
                &cfg(false, false),
                GuestLinearAddress::new(LINEAR),
                AccessKind::Write,
                supervisor
            )),
            ProbeOutcome::Translated(_)
        ));
        let fault = pf(probe_long64_4k(
            &read_only,
            &cfg(true, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Write,
            supervisor,
        ));
        assert_eq!(fault.error_code, 0x03);
        let nx = tables(memory(0x5000), 0x4007 | ENTRY_NX_BIT);
        let fault = pf(probe_long64_4k(
            &nx,
            &cfg(false, true),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Fetch,
            supervisor,
        ));
        assert_eq!(fault.error_code, 0x11);

        let mut upper_user_denied = tables(memory(0x5000), 0x4007);
        upper_user_denied
            .write_u64(GuestPhysicalAddress::new(0x1008), 0x2003, |_| {})
            .unwrap();
        let fault = pf(probe_long64_4k(
            &upper_user_denied,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            user,
        ));
        assert_eq!(fault.error_code, 0x05);
        let mut upper_write_denied = tables(memory(0x5000), 0x4007);
        upper_write_denied
            .write_u64(GuestPhysicalAddress::new(0x1008), 0x2005, |_| {})
            .unwrap();
        let fault = pf(probe_long64_4k(
            &upper_write_denied,
            &cfg(true, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Write,
            supervisor,
        ));
        assert_eq!(fault.error_code, 0x03);
    }

    #[test]
    fn cr3_validation_physical_width_and_table_read_failures_are_explicit() {
        assert!(matches!(
            Long64ProbeConfig::new(0x8000_0023, ROOT, CR4_PAE, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::Cr0EtRequired { value: 0x8000_0023 })
        ));
        assert!(
            Long64ProbeConfig::new(0x8000_0033, ROOT | (1 << 36), CR4_PAE, EFER_LME | EFER_LMA)
                .is_err()
        );
        assert!(Long64ProbeConfig::new(0x8000_0033, ROOT, 0, EFER_LME | EFER_LMA).is_err());
        assert!(Long64ProbeConfig::new(0x8000_0033, ROOT, CR4_PAE, EFER_LME).is_err());
        let ignored_cr3_bits = config(
            0x8000_0033,
            ROOT | 0x18 | 0x7e0,
            CR4_PAE,
            EFER_LME | EFER_LMA,
        );
        let valid = tables(memory(0x5000), 0x4007);
        assert!(matches!(
            result(probe_long64_4k(
                &valid,
                &ignored_cr3_bits,
                GuestLinearAddress::new(LINEAR),
                AccessKind::Read,
                context(PrivilegeLevel::Ring0)
            )),
            ProbeOutcome::Translated(_)
        ));
        let config = cfg(false, false);
        let mut mem = memory(0x2000);
        mem.write_u64(GuestPhysicalAddress::new(0x1008), 0x2007, |_| {})
            .unwrap();
        let report = probe_long64_4k(
            &mem,
            &config,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        );
        assert!(
            matches!(report.outcome, Err(ProbeError::Access(ProbeAccessError::TableRead { level: PageTableLevel::Pdpt, entry_address, .. })) if entry_address.get() == 0x2010)
        );
        assert_eq!(report.trace.len(), 1);
    }

    #[test]
    fn page_table_mutation_is_seen_next_call_and_a_d_bits_are_unchanged() {
        let mem = tables(memory(0x5000), 0x4007 | (1 << 5) | (1 << 6));
        let before = [
            mem.read_u64(GuestPhysicalAddress::new(0x1008)).unwrap(),
            mem.read_u64(GuestPhysicalAddress::new(0x2010)).unwrap(),
            mem.read_u64(GuestPhysicalAddress::new(0x3018)).unwrap(),
            mem.read_u64(GuestPhysicalAddress::new(0x4020)).unwrap(),
        ];
        assert!(matches!(
            result(probe_long64_4k(
                &mem,
                &cfg(false, false),
                GuestLinearAddress::new(LINEAR),
                AccessKind::Write,
                context(PrivilegeLevel::Ring0)
            )),
            ProbeOutcome::Translated(_)
        ));
        assert_eq!(before[3], 0x4007 | (1 << 5) | (1 << 6));
        let after = [
            mem.read_u64(GuestPhysicalAddress::new(0x1008)).unwrap(),
            mem.read_u64(GuestPhysicalAddress::new(0x2010)).unwrap(),
            mem.read_u64(GuestPhysicalAddress::new(0x3018)).unwrap(),
            mem.read_u64(GuestPhysicalAddress::new(0x4020)).unwrap(),
        ];
        assert_eq!(after, before);
        let mut changed = tables(memory(0x5000), 0x5007);
        let first = result(probe_long64_4k(
            &changed,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        assert!(matches!(first, ProbeOutcome::Translated(t) if t.page_base.get() == 0x5000));
        changed
            .write_u64(GuestPhysicalAddress::new(0x4020), 0x6007, |_| {})
            .unwrap();
        let second = result(probe_long64_4k(
            &changed,
            &cfg(false, false),
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        assert!(matches!(second, ProbeOutcome::Translated(t) if t.page_base.get() == 0x6000));
    }
}
