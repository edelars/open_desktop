//! Read-only diagnostic translation probe for the admitted Long64 4 KiB profile.

use crate::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MemoryAccessError, MemoryRegionKind,
    PhysicalMemory,
};
use od_isa::memory::{
    AccessContext, AccessKind, PageFaultRecord, PrivilegeLevel, page_fault_error_code,
};
use std::fmt;

const CR0_PE: u64 = 1 << 0;
const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_ET: u64 = 1 << 4;
const CR0_NE: u64 = 1 << 5;
const CR0_WP: u64 = 1 << 16;
const CR0_AM: u64 = 1 << 18;
const CR0_PG: u64 = 1 << 31;
// CR0.AM only gates the alignment check of CPL3 accesses (together with EFLAGS.AC), which the model
// does not perform, and the walker never reads it: a snapshot-only bit (S4-38L7f).
const CR0_ALLOWED: u64 =
    CR0_PE | CR0_MP | CR0_EM | CR0_TS | CR0_ET | CR0_NE | CR0_WP | CR0_AM | CR0_PG;
const CR4_PAE: u64 = 1 << 5;
const CR4_PSE: u64 = 1 << 4;
const CR4_PGE: u64 = 1 << 7;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
// CR4.PSE does not change IA-32e translation (the PS bit of a PDE is always honored), so it is
// admitted as a snapshot-only bit (S4-38L7c); the walker never reads it. CR4.PGE only enables
// global pages (the G bit of a PTE); the model has no TLB and the walker never reads the G bit
// (bit 8 of a leaf entry), so PGE is likewise a snapshot-only bit (S4-38L7d).
const CR4_ALLOWED: u64 = CR4_PAE | CR4_PSE | CR4_PGE | CR4_OSFXSR | CR4_OSXMMEXCPT;
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
const ENTRY_2M_RESERVED: u64 = 0x1fe000;

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

    /// Builds an immutable diagnostic snapshot from CPU-owned control state.
    /// CR3 remains an explicit argument for callers without modeled CR3; when
    /// the CPU state carries CR3, it must match this snapshot value.
    pub fn from_cpu_state(
        state: &od_isa::state::LongModeState,
        cr3: u64,
        efer: u64,
    ) -> Result<Self, ProbeConfigError> {
        if let Some(cpu_cr3) = state.cr3
            && cpu_cr3 != cr3
        {
            return Err(ProbeConfigError::Cr3Mismatch {
                cpu_cr3,
                snapshot_cr3: cr3,
            });
        }
        let cr0 = state
            .cr0
            .ok_or(ProbeConfigError::MissingCpuRegister { register: "CR0" })?;
        let cr4 = state
            .cr4
            .ok_or(ProbeConfigError::MissingCpuRegister { register: "CR4" })?;
        Self::new(cr0, cr3, cr4, efer)
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
    MissingCpuRegister {
        register: &'static str,
    },
    Cr3Mismatch {
        cpu_cr3: u64,
        snapshot_cr3: u64,
    },
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
        let entry_value = match read_table_entry(memory, entry_address) {
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
                context,
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
        // S4-10c: PS=1 in a PDPTE is reserved in the declared profile (no
        // 1 GiB pages), exactly like PML4E bit 7 from S4-10a.
        if matches!(level, PageTableLevel::Pml4 | PageTableLevel::Pdpt)
            && entry_value & ENTRY_PS_OR_PAT != 0
        {
            offending |= ENTRY_PS_OR_PAT;
        }
        if level == PageTableLevel::Pd && entry_value & ENTRY_PS_OR_PAT != 0 {
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
                context,
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
            context,
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
    context: AccessContext,
    present: bool,
    reserved: bool,
    _level: PageTableLevel,
    reason: PageFaultReason,
    nxe: bool,
) -> PageFaultProbe {
    let error_code = page_fault_error_code(access, context, present, reserved, nxe);
    PageFaultProbe {
        linear: address,
        access,
        page_privilege: context.page_privilege(),
        error_code,
        reason,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstructionProbeOutcome {
    Translated(Translation4K),
    PageFault {
        record: PageFaultRecord,
        reason: PageFaultReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionProbeReport {
    pub outcome: Result<InstructionProbeOutcome, ProbeError>,
    pub trace: WalkTrace,
}

/// One RAM operation performed only after a complete four-level translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RamAccessOperation {
    Fetch,
    Read {
        width: AccessWidth,
    },
    /// One bounded 10-byte LIDT/LGDT pseudo-descriptor read from RAM.
    ReadPseudoDescriptor,
    Write {
        width: AccessWidth,
        value: u64,
    },
}

impl RamAccessOperation {
    const fn access_kind(self) -> AccessKind {
        match self {
            Self::Fetch => AccessKind::Fetch,
            Self::Read { .. } | Self::ReadPseudoDescriptor => AccessKind::Read,
            Self::Write { .. } => AccessKind::Write,
        }
    }

    const fn width(self) -> u8 {
        match self {
            Self::Fetch => 1,
            Self::Read { width } | Self::Write { width, .. } => width.bytes(),
            Self::ReadPseudoDescriptor => 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackedAccessRequest {
    pub instruction_rip: u64,
    pub address: GuestLinearAddress,
    pub context: AccessContext,
    pub operation: RamAccessOperation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdBit {
    Accessed,
    Dirty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdUpdate {
    pub level: PageTableLevel,
    pub address: GuestPhysicalAddress,
    pub before: u64,
    pub after: u64,
    pub bit: AdBit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackedAccessOutcome {
    Fetched(u8),
    Read(u64),
    ReadPseudoDescriptor([u8; 10]),
    Written,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedAccessError {
    CrossesPage {
        address: GuestLinearAddress,
        width: u8,
    },
    NonCanonical(NonCanonicalAddress),
    UnsupportedPageSize {
        level: PageTableLevel,
        entry_address: GuestPhysicalAddress,
        entry_value: u64,
    },
    PageFault {
        record: PageFaultRecord,
        reason: PageFaultReason,
    },
    TableRead {
        level: PageTableLevel,
        entry_address: GuestPhysicalAddress,
        source: MemoryAccessError,
    },
    TableWrite {
        level: PageTableLevel,
        entry_address: GuestPhysicalAddress,
        bit: AdBit,
        source: MemoryAccessError,
    },
    OperandPhysical {
        address: GuestPhysicalAddress,
        source: MemoryAccessError,
    },
    /// The RAM-only tracked path does not dispatch physical MMIO accesses.
    DeviceUnavailable {
        address: GuestPhysicalAddress,
    },
    MmioBackend {
        address: GuestPhysicalAddress,
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MmioDispatchError {
    Backend(String),
}

type MmioDispatch<'a> =
    dyn FnMut(GuestPhysicalAddress, u8, RamAccessOperation) -> Result<u64, MmioDispatchError> + 'a;

/// The report retains the walk trace and committed A/D writes. Notifications
/// for all committed writes, including the final operand, go only to the caller
/// supplied callback and are not copied into this report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedAccessReport {
    pub outcome: Result<TrackedAccessOutcome, TrackedAccessError>,
    pub trace: WalkTrace,
    pub ad_updates: [Option<AdUpdate>; 5],
    pub ad_update_count: usize,
}

#[derive(Clone, Copy)]
struct WalkTranslation {
    physical: GuestPhysicalAddress,
    leaf_address: GuestPhysicalAddress,
    leaf_value: u64,
    leaf_level: PageTableLevel,
    /// Physical bases of the tables visited (PML4 first); levels not visited
    /// (past a 2 MiB leaf) are `None`.
    table_pages: [Option<GuestPhysicalAddress>; 4],
}

struct WalkResult {
    translation: Result<WalkTranslation, TrackedAccessError>,
    trace: WalkTrace,
    ad_updates: [Option<AdUpdate>; 4],
    ad_update_count: usize,
}

type TrackedTableWriter<'a> = dyn FnMut(
        &mut PhysicalMemory,
        GuestPhysicalAddress,
        u64,
        &mut dyn FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError>
    + 'a;

fn write_tracked_table_entry(
    memory: &mut PhysicalMemory,
    address: GuestPhysicalAddress,
    value: u64,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
) -> Result<(), MemoryAccessError> {
    memory.write_u64(address, value, committed_write)
}

fn walk_long64_4k(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    address: GuestLinearAddress,
    context: AccessContext,
    access: AccessKind,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
) -> WalkResult {
    walk_long64_4k_with_writer(
        memory,
        paging,
        instruction_rip,
        address,
        context,
        access,
        committed_write,
        &mut write_tracked_table_entry,
        false,
    )
}

fn walk_long64_4k_with_writer(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    address: GuestLinearAddress,
    context: AccessContext,
    access: AccessKind,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
    table_writer: &mut TrackedTableWriter<'_>,
    allow_2m: bool,
) -> WalkResult {
    let mut trace = WalkTrace::new();
    let mut ad_updates = [None; 4];
    let mut ad_update_count = 0;
    let fail = |error, trace, ad_updates, ad_update_count| WalkResult {
        translation: Err(error),
        trace,
        ad_updates,
        ad_update_count,
    };
    if !is_canonical_48(address.0) {
        return fail(
            TrackedAccessError::NonCanonical(NonCanonicalAddress { linear: address }),
            trace,
            ad_updates,
            ad_update_count,
        );
    }

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
    let mut leaf = (GuestPhysicalAddress::new(0), 0u64);
    let mut large_leaf = false;
    let mut table_pages = [None; 4];
    for (table_index, level) in levels.into_iter().enumerate() {
        table_pages[table_index] = Some(GuestPhysicalAddress::new(table_base));
        let index = ((address.0 >> level.index_shift()) & 0x1ff) as u16;
        let entry_address = GuestPhysicalAddress::new(table_base + u64::from(index) * 8);
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
        trace.push(WalkEntry {
            level,
            index,
            entry_address,
            entry_value,
        });
        if entry_value & ENTRY_PRESENT == 0 {
            let reason = PageFaultReason::NotPresent { level };
            let record = PageFaultRecord::new(
                instruction_rip,
                access,
                context,
                page_fault_error_code(access, context, false, false, paging.nxe()),
                address.0,
            );
            return fail(
                TrackedAccessError::PageFault { record, reason },
                trace,
                ad_updates,
                ad_update_count,
            );
        }
        let mut offending = entry_value & ENTRY_ADDRESS_RESERVED;
        if !paging.nxe() {
            offending |= entry_value & ENTRY_NX;
        }
        // S4-10c: PS=1 in a PDPTE is reserved in the declared profile (no
        // 1 GiB pages), exactly like PML4E bit 7 from S4-10a.
        if matches!(level, PageTableLevel::Pml4 | PageTableLevel::Pdpt)
            && entry_value & ENTRY_PS_OR_PAT != 0
        {
            offending |= ENTRY_PS_OR_PAT;
        }
        let is_large_leaf =
            allow_2m && level == PageTableLevel::Pd && entry_value & ENTRY_PS_OR_PAT != 0;
        if is_large_leaf {
            // Bits 20:13 of a 2 MiB PDE are reserved (address bits below 2 MiB).
            offending |= entry_value & ENTRY_2M_RESERVED;
        }
        if level == PageTableLevel::Pd && entry_value & ENTRY_PS_OR_PAT != 0 && !is_large_leaf {
            return fail(
                TrackedAccessError::UnsupportedPageSize {
                    level,
                    entry_address,
                    entry_value,
                },
                trace,
                ad_updates,
                ad_update_count,
            );
        }
        if offending != 0 {
            let reason = PageFaultReason::ReservedBits {
                level,
                offending_bits: offending,
            };
            let record = PageFaultRecord::new(
                instruction_rip,
                access,
                context,
                page_fault_error_code(access, context, true, true, paging.nxe()),
                address.0,
            );
            return fail(
                TrackedAccessError::PageFault { record, reason },
                trace,
                ad_updates,
                ad_update_count,
            );
        }

        if entry_value & (1 << 5) == 0 {
            let after = entry_value | (1 << 5);
            if let Err(source) = table_writer(memory, entry_address, after, &mut *committed_write) {
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
            ad_updates[ad_update_count] = Some(AdUpdate {
                level,
                address: entry_address,
                before: entry_value,
                after,
                bit: AdBit::Accessed,
            });
            ad_update_count += 1;
        }
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
        let reason = PageFaultReason::Protection {
            user_denied,
            write_denied,
            execute_denied,
        };
        let record = PageFaultRecord::new(
            instruction_rip,
            access,
            context,
            page_fault_error_code(access, context, true, false, paging.nxe()),
            address.0,
        );
        return fail(
            TrackedAccessError::PageFault { record, reason },
            trace,
            ad_updates,
            ad_update_count,
        );
    }
    let offset_mask = if large_leaf { 0x1f_ffff } else { 0xfff };
    let physical = GuestPhysicalAddress::new(
        (leaf.1 & ENTRY_ADDRESS_MASK & !offset_mask) | (address.0 & offset_mask),
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

/// Performs one same-page fetch/read/write through RAM page tables and RAM only.
/// A request crossing a 4 KiB boundary is rejected before table or operand
/// access. Separate calls commit independently; this API does not split or
/// transact a multi-page operation.
/// A/D updates commit immediately through `PhysicalMemory::write_u64`; those
/// effects remain committed if a later walk or operand access fails.
pub fn access_long64_4k_ram(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> TrackedAccessReport {
    access_long64_4k_internal(memory, paging, request, committed_write, false, None, false)
}

/// Opt-in tracked access through 4 KiB or 2 MiB leaves, limited to RAM.
///
/// Behaves like [`access_long64_4k_ram`], except that a PDE with PS=1 is a
/// 2 MiB leaf instead of `UnsupportedPageSize`. The 4 KiB operand-crossing
/// rule is unchanged: a request crossing a 4 KiB boundary is still rejected.
pub fn access_long64_2m_ram(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> TrackedAccessReport {
    access_long64_4k_internal(memory, paging, request, committed_write, false, None, true)
}

/// Translates `address` through the 4 KiB / 2 MiB tracked walk WITHOUT committing anything: the
/// table writer is a no-op, so no accessed/dirty bit changes in guest RAM. The checks are those of
/// [`access_long64_2m_ram`] (S4-38L1: the stop record reads the bytes at RIP with the walk the
/// guest itself fetched with, and must not alter the page tables it reports on).
pub(crate) fn translate_long64_2m_ram_read_only(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    address: GuestLinearAddress,
    context: AccessContext,
    access: AccessKind,
) -> Result<GuestPhysicalAddress, TrackedAccessError> {
    translate_long64_2m_ram_read_only_with_tables(
        memory,
        paging,
        instruction_rip,
        address,
        context,
        access,
    )
    .map(|(physical, _)| physical)
}

/// [`translate_long64_2m_ram_read_only`] that also returns the physical bases of the tables the
/// walk visited (PML4 first; `None` past a 2 MiB leaf). S4-13: the fetch translation cache
/// watches exactly those frames for guest writes.
pub(crate) fn translate_long64_2m_ram_read_only_with_tables(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    address: GuestLinearAddress,
    context: AccessContext,
    access: AccessKind,
) -> Result<(GuestPhysicalAddress, [Option<GuestPhysicalAddress>; 4]), TrackedAccessError> {
    let mut no_op_writer = |_: &mut PhysicalMemory,
                            _: GuestPhysicalAddress,
                            _: u64,
                            _: &mut dyn FnMut(GuestPhysicalRange)| Ok(());
    walk_long64_4k_with_writer(
        memory,
        paging,
        instruction_rip,
        address,
        context,
        access,
        &mut |_| {},
        &mut no_op_writer,
        true,
    )
    .translation
    .map(|translation| (translation.physical, translation.table_pages))
}

/// S4-38L4h: the translation half of a data write through the 4 KiB / 2 MiB tracked walk, with no
/// operand access. The walk is the one a real write takes: the present, reserved-bit, user and
/// write-protection checks are those of [`access_long64_2m_ram`] for a write, and accessed bits
/// are set (and reported through `committed_write`) exactly as a walk does. The DIRTY bit is not
/// set and no operand byte is written: a split store calls this for both of its pages first and
/// writes only if both succeed, so a fault on the second page leaves the first untouched. The
/// target must be plain RAM (a write to MMIO or a read-only region is a typed error, never
/// started on the first page and abandoned on the second).
pub(crate) fn translate_long64_2m_ram_write_check(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    address: GuestLinearAddress,
    context: AccessContext,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<(), TrackedAccessError> {
    let walk = walk_long64_4k_with_writer(
        memory,
        paging,
        instruction_rip,
        address,
        context,
        AccessKind::Write,
        &mut committed_write,
        &mut write_tracked_table_entry,
        true,
    );
    let physical = walk.translation?.physical;
    match memory.locate_region_kind(physical, 1) {
        Ok(MemoryRegionKind::Ram | MemoryRegionKind::Framebuffer) => Ok(()),
        Ok(MemoryRegionKind::Mmio) => {
            Err(TrackedAccessError::DeviceUnavailable { address: physical })
        }
        Ok(kind) => Err(TrackedAccessError::OperandPhysical {
            address: physical,
            source: MemoryAccessError::ReadOnlyRegion(kind),
        }),
        Err(source) => Err(TrackedAccessError::OperandPhysical {
            address: physical,
            source,
        }),
    }
}

pub(crate) fn access_long64_4k_with_mmio(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    committed_write: impl FnMut(GuestPhysicalRange),
    mut mmio: impl FnMut(GuestPhysicalAddress, u8, RamAccessOperation) -> Result<u64, MmioDispatchError>,
) -> TrackedAccessReport {
    access_long64_4k_internal(
        memory,
        paging,
        request,
        committed_write,
        true,
        Some(&mut mmio),
        false,
    )
}

/// S4-38L9s: [`access_long64_2m_ram`] whose MMIO operands go to `mmio` instead of failing
/// (the set loop's data path). The walk, accessed/dirty bits and every check are those of the
/// 2 MiB RAM walk; only an operand landing in an MMIO region is dispatched, with the dirty bit set
/// first like [`access_long64_4k_with_mmio`].
pub(crate) fn access_long64_2m_with_mmio(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    committed_write: impl FnMut(GuestPhysicalRange),
    mut mmio: impl FnMut(GuestPhysicalAddress, u8, RamAccessOperation) -> Result<u64, MmioDispatchError>,
) -> TrackedAccessReport {
    access_long64_4k_internal(
        memory,
        paging,
        request,
        committed_write,
        true,
        Some(&mut mmio),
        true,
    )
}

fn access_long64_4k_internal(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    mut committed_write: impl FnMut(GuestPhysicalRange),
    set_mmio_dirty_before_dispatch: bool,
    mmio: Option<&mut MmioDispatch<'_>>,
    allow_2m: bool,
) -> TrackedAccessReport {
    access_long64_4k_internal_with_writer(
        memory,
        paging,
        request,
        &mut committed_write,
        set_mmio_dirty_before_dispatch,
        mmio,
        &mut write_tracked_table_entry,
        allow_2m,
    )
}

fn access_long64_4k_internal_with_writer(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
    set_mmio_dirty_before_dispatch: bool,
    mut mmio: Option<&mut MmioDispatch<'_>>,
    table_writer: &mut TrackedTableWriter<'_>,
    allow_2m: bool,
) -> TrackedAccessReport {
    let mut trace = WalkTrace::new();
    let mut ad_updates = [None; 5];
    let mut ad_update_count = 0;
    let width = request.operation.width();
    let address = request.address;
    let access = request.operation.access_kind();
    let fail = |outcome, trace, ad_updates, ad_update_count| TrackedAccessReport {
        outcome: Err(outcome),
        trace,
        ad_updates,
        ad_update_count,
    };

    if u16::from((address.0 & 0xfff) as u16) + u16::from(width) > 4096 {
        return fail(
            TrackedAccessError::CrossesPage { address, width },
            trace,
            ad_updates,
            ad_update_count,
        );
    }
    let walk = walk_long64_4k_with_writer(
        memory,
        paging,
        request.instruction_rip,
        address,
        request.context,
        access,
        committed_write,
        table_writer,
        allow_2m,
    );
    trace = walk.trace;
    ad_update_count = walk.ad_update_count;
    ad_updates[..ad_update_count].copy_from_slice(&walk.ad_updates[..ad_update_count]);
    let translation = match walk.translation {
        Ok(translation) => translation,
        Err(error) => return fail(error, trace, ad_updates, ad_update_count),
    };
    let leaf = (translation.leaf_address, translation.leaf_value);
    let physical = translation.physical;
    let range = match GuestPhysicalRange::new(physical, u64::from(width)) {
        Ok(range) => range,
        Err(source) => {
            return fail(
                TrackedAccessError::OperandPhysical {
                    address: physical,
                    source,
                },
                trace,
                ad_updates,
                ad_update_count,
            );
        }
    };
    let is_mmio = match memory.locate_region_kind(physical, u64::from(width)) {
        Ok(MemoryRegionKind::Ram) => false,
        Ok(MemoryRegionKind::Mmio) => true,
        // Data accesses behave like RAM (the region itself suppresses the
        // committed-write callback); instruction fetch is a backend error.
        Ok(MemoryRegionKind::Framebuffer) if request.operation != RamAccessOperation::Fetch => {
            false
        }
        Ok(kind) => {
            return fail(
                TrackedAccessError::OperandPhysical {
                    address: physical,
                    source: if access == AccessKind::Write {
                        MemoryAccessError::ReadOnlyRegion(kind)
                    } else {
                        MemoryAccessError::UnreadableRegion(kind)
                    },
                },
                trace,
                ad_updates,
                ad_update_count,
            );
        }
        Err(source) => {
            return fail(
                TrackedAccessError::OperandPhysical {
                    address: physical,
                    source,
                },
                trace,
                ad_updates,
                ad_update_count,
            );
        }
    };
    let _ = range;

    if is_mmio && mmio.is_none() {
        let error = if access == AccessKind::Write {
            TrackedAccessError::DeviceUnavailable { address: physical }
        } else {
            TrackedAccessError::OperandPhysical {
                address: physical,
                source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Mmio),
            }
        };
        return fail(error, trace, ad_updates, ad_update_count);
    }

    if let RamAccessOperation::Write { width, value } = request.operation {
        if !is_mmio || set_mmio_dirty_before_dispatch {
            if leaf.1 & (1 << 6) == 0 {
                let after = leaf.1 | (1 << 6);
                if let Err(source) = table_writer(memory, leaf.0, after, committed_write) {
                    return fail(
                        TrackedAccessError::TableWrite {
                            level: translation.leaf_level,
                            entry_address: leaf.0,
                            bit: AdBit::Dirty,
                            source,
                        },
                        trace,
                        ad_updates,
                        ad_update_count,
                    );
                }
                ad_updates[ad_update_count] = Some(AdUpdate {
                    level: translation.leaf_level,
                    address: leaf.0,
                    before: leaf.1,
                    after,
                    bit: AdBit::Dirty,
                });
                ad_update_count += 1;
            }
        }
        if is_mmio {
            return match mmio.as_mut().expect("MMIO dispatch was validated")(
                physical,
                width.bytes(),
                request.operation,
            ) {
                Ok(_) => TrackedAccessReport {
                    outcome: Ok(TrackedAccessOutcome::Written),
                    trace,
                    ad_updates,
                    ad_update_count,
                },
                Err(MmioDispatchError::Backend(message)) => fail(
                    TrackedAccessError::MmioBackend {
                        address: physical,
                        message,
                    },
                    trace,
                    ad_updates,
                    ad_update_count,
                ),
            };
        }
        let bytes = value.to_le_bytes();
        if let Err(source) = memory.write_bytes(
            physical,
            &bytes[..usize::from(width.bytes())],
            &mut *committed_write,
        ) {
            return fail(
                TrackedAccessError::OperandPhysical {
                    address: physical,
                    source,
                },
                trace,
                ad_updates,
                ad_update_count,
            );
        }
        return TrackedAccessReport {
            outcome: Ok(TrackedAccessOutcome::Written),
            trace,
            ad_updates,
            ad_update_count,
        };
    }
    if is_mmio {
        return match mmio.as_mut().expect("MMIO dispatch was validated")(
            physical,
            width,
            request.operation,
        ) {
            Ok(value) => TrackedAccessReport {
                outcome: Ok(TrackedAccessOutcome::Read(value)),
                trace,
                ad_updates,
                ad_update_count,
            },
            Err(MmioDispatchError::Backend(message)) => fail(
                TrackedAccessError::MmioBackend {
                    address: physical,
                    message,
                },
                trace,
                ad_updates,
                ad_update_count,
            ),
        };
    }
    if request.operation == RamAccessOperation::Fetch {
        return match memory.read_u8(physical) {
            Ok(value) => TrackedAccessReport {
                outcome: Ok(TrackedAccessOutcome::Fetched(value)),
                trace,
                ad_updates,
                ad_update_count,
            },
            Err(source) => fail(
                TrackedAccessError::OperandPhysical {
                    address: physical,
                    source,
                },
                trace,
                ad_updates,
                ad_update_count,
            ),
        };
    }
    if request.operation == RamAccessOperation::ReadPseudoDescriptor {
        let mut bytes = [0u8; 10];
        return match memory.read_bytes(physical, &mut bytes) {
            Ok(()) => TrackedAccessReport {
                outcome: Ok(TrackedAccessOutcome::ReadPseudoDescriptor(bytes)),
                trace,
                ad_updates,
                ad_update_count,
            },
            Err(source) => fail(
                TrackedAccessError::OperandPhysical {
                    address: physical,
                    source,
                },
                trace,
                ad_updates,
                ad_update_count,
            ),
        };
    }
    let mut bytes = [0u8; 8];
    let len = usize::from(width);
    match memory.read_bytes(physical, &mut bytes[..len]) {
        Ok(()) => {
            let mut value_bytes = [0u8; 8];
            value_bytes[..len].copy_from_slice(&bytes[..len]);
            TrackedAccessReport {
                outcome: Ok(TrackedAccessOutcome::Read(u64::from_le_bytes(value_bytes))),
                trace,
                ad_updates,
                ad_update_count,
            }
        }
        Err(source) => fail(
            TrackedAccessError::OperandPhysical {
                address: physical,
                source,
            },
            trace,
            ad_updates,
            ad_update_count,
        ),
    }
}

/// Errors from the bounded paged LTR descriptor transaction. Architectural
/// descriptor faults and guest `#PF` are part of the guest exception model; the
/// remaining variants are bounded-profile rejections.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum LtrTransactionError {
    Access(TrackedAccessError),
    TableAlias,
    TranslationChanged,
    Descriptor(od_isa::memory::ArchitecturalFault),
}

fn descriptor_ram_error(
    memory: &PhysicalMemory,
    physical: GuestPhysicalAddress,
    write: bool,
) -> Result<(), TrackedAccessError> {
    let physical_error = |source| TrackedAccessError::OperandPhysical {
        address: physical,
        source,
    };
    match memory.locate_region_kind(physical, 16) {
        Ok(MemoryRegionKind::Ram) => Ok(()),
        Ok(MemoryRegionKind::Mmio) => {
            Err(TrackedAccessError::DeviceUnavailable { address: physical })
        }
        Ok(kind) => Err(physical_error(if write {
            MemoryAccessError::ReadOnlyRegion(kind)
        } else {
            MemoryAccessError::UnreadableRegion(kind)
        })),
        Err(source) => Err(physical_error(source)),
    }
}

/// Owner-exclusive read, validate and busy-bit commit of one 16-byte GDT TSS
/// descriptor: read walk, validation, write-permission walk, then the leaf D
/// update and the descriptor image write. Nothing is written before every
/// check passes; page-table A bits (and their callbacks) from the walks remain
/// committed when a later check fails. The caller must exclude DMA, other
/// vCPUs and host writers for the duration of the call.
pub(crate) fn commit_paged_ltr_descriptor(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    state: &od_isa::LongModeState,
    preparation: &od_isa::execute::PagedLtrPreparation,
    context: AccessContext,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<od_isa::execute::PreparedPagedLtrCommit, LtrTransactionError> {
    let od_isa::execute::PagedLtrPreparation::Ready {
        descriptor_address, ..
    } = preparation
    else {
        unreachable!("caller passes only ready preparations")
    };
    let address = GuestLinearAddress::new(*descriptor_address);
    let context = context.descriptor_table();
    if address.0 & 0xfff > 0xff0 {
        return Err(LtrTransactionError::Access(
            TrackedAccessError::CrossesPage { address, width: 16 },
        ));
    }
    let read = walk_long64_4k(
        memory,
        paging,
        state.rip,
        address,
        context,
        AccessKind::Read,
        &mut committed_write,
    )
    .translation
    .map_err(LtrTransactionError::Access)?;
    let page = read.physical.get() & !0xfff;
    if read.table_pages.iter().flatten().any(|p| p.get() == page) {
        return Err(LtrTransactionError::TableAlias);
    }
    descriptor_ram_error(memory, read.physical, false).map_err(LtrTransactionError::Access)?;
    let mut snapshot = [0u8; 16];
    memory
        .read_bytes(read.physical, &mut snapshot)
        .map_err(|source| {
            LtrTransactionError::Access(TrackedAccessError::OperandPhysical {
                address: read.physical,
                source,
            })
        })?;
    let prepared = od_isa::execute::prepare_paged_ltr_commit(
        preparation,
        context.privilege,
        state.gdtr.expect("ready preparation requires GDTR"),
        snapshot,
    )
    .map_err(LtrTransactionError::Descriptor)?
    .expect("ready preparation");

    // Write-permission walk: sets A bits but not D, and never writes the operand.
    let write = walk_long64_4k(
        memory,
        paging,
        state.rip,
        address,
        context,
        AccessKind::Write,
        &mut committed_write,
    )
    .translation
    .map_err(LtrTransactionError::Access)?;
    if read.physical != write.physical
        || read.leaf_address != write.leaf_address
        || read.table_pages != write.table_pages
    {
        return Err(LtrTransactionError::TranslationChanged);
    }
    let page = write.physical.get() & !0xfff;
    if write.table_pages.iter().flatten().any(|p| p.get() == page) {
        return Err(LtrTransactionError::TableAlias);
    }
    descriptor_ram_error(memory, write.physical, true).map_err(LtrTransactionError::Access)?;

    if write.leaf_value & (1 << 6) == 0 {
        let after = write.leaf_value | (1 << 6);
        memory
            .write_u64(write.leaf_address, after, &mut committed_write)
            .map_err(|source| {
                LtrTransactionError::Access(TrackedAccessError::TableWrite {
                    level: write.leaf_level,
                    entry_address: write.leaf_address,
                    bit: AdBit::Dirty,
                    source,
                })
            })?;
    }
    memory
        .write_bytes(
            write.physical,
            prepared.busy_descriptor(),
            &mut committed_write,
        )
        .map_err(|source| {
            LtrTransactionError::Access(TrackedAccessError::OperandPhysical {
                address: write.physical,
                source,
            })
        })?;
    Ok(prepared)
}

pub(crate) fn read_long64_4k_bounded_ram_read(
    memory: &mut PhysicalMemory,
    paging: &Long64ProbeConfig,
    request: TrackedAccessRequest,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<u64, TrackedAccessError> {
    let (width, access) = match request.operation {
        RamAccessOperation::Read {
            width: width @ (AccessWidth::Word | AccessWidth::Dword | AccessWidth::Qword),
        } => (width.bytes(), AccessKind::Read),
        RamAccessOperation::Write { width, .. } => {
            return Err(TrackedAccessError::CrossesPage {
                address: request.address,
                width: width.bytes(),
            });
        }
        operation => {
            return Err(TrackedAccessError::CrossesPage {
                address: request.address,
                width: operation.width(),
            });
        }
    };
    let offset = request.address.0 & 0xfff;
    let first_len = (4096 - offset).min(u64::from(width)) as u8;
    let crosses_page = first_len < width;
    let first = walk_long64_4k(
        memory,
        paging,
        request.instruction_rip,
        request.address,
        request.context,
        access,
        &mut committed_write,
    )
    .translation?;
    let second = if crosses_page {
        let second_linear = request.address.0.checked_add(u64::from(first_len)).ok_or(
            TrackedAccessError::CrossesPage {
                address: request.address,
                width,
            },
        )?;
        Some(
            walk_long64_4k(
                memory,
                paging,
                request.instruction_rip,
                GuestLinearAddress::new(second_linear),
                request.context,
                access,
                &mut committed_write,
            )
            .translation?,
        )
    } else {
        None
    };
    let second_len = width - first_len;
    let check_data_backed =
        |physical, len| match memory.locate_region_kind(physical, u64::from(len)) {
            Ok(MemoryRegionKind::Ram | MemoryRegionKind::Framebuffer) => Ok(()),
            Ok(kind) => Err(TrackedAccessError::OperandPhysical {
                address: physical,
                source: MemoryAccessError::UnreadableRegion(kind),
            }),
            Err(source) => Err(TrackedAccessError::OperandPhysical {
                address: physical,
                source,
            }),
        };
    check_data_backed(first.physical, first_len)?;
    if let Some(second) = second {
        check_data_backed(second.physical, second_len)?;
    }

    let mut bytes = [0; 8];
    memory
        .read_bytes(first.physical, &mut bytes[..usize::from(first_len)])
        .map_err(|source| TrackedAccessError::OperandPhysical {
            address: first.physical,
            source,
        })?;
    if let Some(second) = second {
        memory
            .read_bytes(
                second.physical,
                &mut bytes[usize::from(first_len)..usize::from(width)],
            )
            .map_err(|source| TrackedAccessError::OperandPhysical {
                address: second.physical,
                source,
            })?;
    }
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tracked_access_tests {
    use super::*;
    use crate::{MachineMemoryMap, MemoryRegionSpec, RamLimits};
    use od_isa::memory::{AccessOrigin, InstructionMode};

    const LINEAR: u64 = 0x0000_0080_8060_4abc;
    const RIP: u64 = 0x1234;

    fn setup(entry: u64, size: u64) -> (PhysicalMemory, Long64ProbeConfig) {
        setup_entries([0x2007, 0x3007, 0x4007, entry], size)
    }

    fn setup_entries(entries: [u64; 4], size: u64) -> (PhysicalMemory, Long64ProbeConfig) {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), size).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(size)).unwrap();
        for (address, value) in [0x1008, 0x2010, 0x3018, 0x4020].into_iter().zip(entries) {
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        let paging =
            Long64ProbeConfig::new(0x8000_0033, 0x1000, CR4_PAE, EFER_LME | EFER_LMA | EFER_NXE)
                .unwrap();
        (memory, paging)
    }

    fn request(operation: RamAccessOperation) -> TrackedAccessRequest {
        request_as(
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
            operation,
        )
    }

    fn request_as(context: AccessContext, operation: RamAccessOperation) -> TrackedAccessRequest {
        TrackedAccessRequest {
            instruction_rip: RIP,
            address: GuestLinearAddress::new(LINEAR),
            context,
            operation,
        }
    }

    fn operation(access: AccessKind) -> RamAccessOperation {
        match access {
            AccessKind::Read => RamAccessOperation::Read {
                width: AccessWidth::Byte,
            },
            AccessKind::Write => RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xa5,
            },
            AccessKind::Fetch => RamAccessOperation::Fetch,
        }
    }

    fn privilege(level: PrivilegeLevel) -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: level,
            origin: AccessOrigin::Explicit,
        }
    }

    fn paging_config(wp: bool, nxe: bool) -> Long64ProbeConfig {
        Long64ProbeConfig::new(
            0x8000_0033 | if wp { CR0_WP } else { 0 },
            0x1000,
            CR4_PAE,
            EFER_LME | EFER_LMA | if nxe { EFER_NXE } else { 0 },
        )
        .unwrap()
    }

    fn entries(memory: &PhysicalMemory) -> [u64; 4] {
        [0x1008, 0x2010, 0x3018, 0x4020]
            .map(|address| memory.read_u64(GuestPhysicalAddress::new(address)).unwrap())
    }

    fn logged(log: &[GuestPhysicalRange]) -> Vec<(u64, u64)> {
        log.iter()
            .map(|range| (range.start().get(), range.len()))
            .collect()
    }

    /// S4-10c: literal supervisor PFEC of a reserved-bit fault (P|RSVD, plus W
    /// for writes and I/D for fetches only when NXE=1), per Intel SDM vol. 3
    /// "Page-Fault Error Code". Deliberately independent of the code under test.
    fn rsvd_pfec(access: AccessKind, nxe: bool) -> u32 {
        match (access, nxe) {
            (AccessKind::Read, _) | (AccessKind::Fetch, false) => 0x09,
            (AccessKind::Write, _) => 0x0b,
            (AccessKind::Fetch, true) => 0x19,
        }
    }

    /// Expected pure-probe outcome for a present PS=1 entry at `level`.
    /// PDPTE.PS=1 is a guest reserved-bit #PF (S4-10c: replaces the S4-10a
    /// host error); PDE.PS=1 stays `UnsupportedPageSize` in the 4 KiB profile.
    /// `extra_offending` is the other reserved bits set in the entry.
    fn ps_probe_expected(
        level: PageTableLevel,
        entry_address: u64,
        entry_value: u64,
        access: AccessKind,
        nxe: bool,
        extra_offending: u64,
    ) -> Result<ProbeOutcome, ProbeError> {
        if level == PageTableLevel::Pdpt {
            Ok(ProbeOutcome::PageFault(PageFaultProbe {
                linear: GuestLinearAddress::new(LINEAR),
                access,
                page_privilege: PrivilegeLevel::Ring0,
                error_code: rsvd_pfec(access, nxe),
                reason: PageFaultReason::ReservedBits {
                    level,
                    offending_bits: ENTRY_PS_OR_PAT | extra_offending,
                },
            }))
        } else {
            Err(ProbeError::UnsupportedPageSize {
                level,
                entry_address: GuestPhysicalAddress::new(entry_address),
                entry_value,
            })
        }
    }

    /// Tracked counterpart of [`ps_probe_expected`]; CR2 value is `LINEAR`.
    fn ps_tracked_expected(
        level: PageTableLevel,
        entry_address: u64,
        entry_value: u64,
        operation: RamAccessOperation,
        nxe: bool,
        extra_offending: u64,
    ) -> Result<TrackedAccessOutcome, TrackedAccessError> {
        if level == PageTableLevel::Pdpt {
            let access = operation.access_kind();
            Err(TrackedAccessError::PageFault {
                record: PageFaultRecord::new(
                    RIP,
                    access,
                    request(operation).context,
                    rsvd_pfec(access, nxe),
                    LINEAR,
                ),
                reason: PageFaultReason::ReservedBits {
                    level,
                    offending_bits: ENTRY_PS_OR_PAT | extra_offending,
                },
            })
        } else {
            Err(TrackedAccessError::UnsupportedPageSize {
                level,
                entry_address: GuestPhysicalAddress::new(entry_address),
                entry_value,
            })
        }
    }

    fn ram_snapshot(memory: &PhysicalMemory, start: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        memory
            .read_bytes(GuestPhysicalAddress::new(start), &mut bytes)
            .unwrap();
        bytes
    }

    fn expected_accessed_updates(count: usize) -> [Option<AdUpdate>; 5] {
        let expected = [
            (PageTableLevel::Pml4, 0x1008, 0x2007, 0x2027),
            (PageTableLevel::Pdpt, 0x2010, 0x3007, 0x3027),
            (PageTableLevel::Pd, 0x3018, 0x4007, 0x4027),
            (PageTableLevel::Pt, 0x4020, 0x4007, 0x4027),
        ];
        std::array::from_fn(|index| {
            if index >= count {
                return None;
            }
            let (level, address, before, after) = expected[index];
            Some(AdUpdate {
                level,
                address: GuestPhysicalAddress::new(address),
                before,
                after,
                bit: AdBit::Accessed,
            })
        })
    }

    fn expected_matrix_updates(entries: [u64; 4], count: usize) -> [Option<AdUpdate>; 5] {
        let levels = [
            PageTableLevel::Pml4,
            PageTableLevel::Pdpt,
            PageTableLevel::Pd,
            PageTableLevel::Pt,
        ];
        let addresses = [0x1008, 0x2010, 0x3018, 0x4020];
        std::array::from_fn(|index| {
            (index < count && index < 4).then(|| AdUpdate {
                level: levels[index],
                address: GuestPhysicalAddress::new(addresses[index]),
                before: entries[index],
                after: entries[index] | (1 << 5),
                bit: AdBit::Accessed,
            })
        })
    }

    fn matrix_entries_with_bit(index: usize, bit: u64, set: bool) -> [u64; 4] {
        let mut entries = [0x2007, 0x3007, 0x4007, 0x4007];
        if set {
            entries[index] |= bit;
        } else {
            entries[index] &= !bit;
        }
        entries
    }

    fn level(index: usize) -> PageTableLevel {
        [
            PageTableLevel::Pml4,
            PageTableLevel::Pdpt,
            PageTableLevel::Pd,
            PageTableLevel::Pt,
        ][index]
    }

    fn assert_matrix_fault(
        initial_entries: [u64; 4],
        paging: Long64ProbeConfig,
        context: AccessContext,
        operation: RamAccessOperation,
        fault_level: usize,
        error_code: u32,
        reason: PageFaultReason,
    ) {
        let trace_count = match &reason {
            PageFaultReason::NotPresent { .. } | PageFaultReason::ReservedBits { .. } => {
                fault_level + 1
            }
            PageFaultReason::Protection { .. } => 4,
        };
        let (mut memory, _) = setup_entries(initial_entries, 0x6000);
        memory
            .write_u8(GuestPhysicalAddress::new(0x4abc), 0x5a, |_| {})
            .unwrap();
        let operand_before = ram_snapshot(&memory, 0x4abc, 8);
        let mut callbacks = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request_as(context, operation),
            |range| callbacks.push(range),
        );
        let fault = PageFaultRecord::new(RIP, operation.access_kind(), context, error_code, LINEAR);
        assert_eq!(
            &report.outcome,
            &Err(TrackedAccessError::PageFault {
                record: fault,
                reason,
            })
        );
        assert_eq!(report.trace.len(), trace_count);
        assert_eq!(report.ad_update_count, fault_level);
        assert_eq!(
            report.ad_updates,
            expected_matrix_updates(initial_entries, fault_level)
        );
        let addresses = [0x1008, 0x2010, 0x3018, 0x4020];
        assert_eq!(
            logged(&callbacks),
            addresses[..fault_level]
                .iter()
                .map(|address| (*address, 8))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            entries(&memory),
            std::array::from_fn(|index| {
                initial_entries[index] | if index < fault_level { 1 << 5 } else { 0 }
            })
        );
        assert_eq!(ram_snapshot(&memory, 0x4abc, 8), operand_before);
        let record = match report.outcome.as_ref().unwrap_err() {
            TrackedAccessError::PageFault { record, .. } => record,
            other => panic!("expected page fault, got {other:?}"),
        };
        assert_eq!(record.fault().vector, 14);
        assert_eq!(record.fault().error_code, Some(error_code));
        assert_eq!(record.fault().linear_address, Some(LINEAR));
        assert_eq!(record.saved_rip(), RIP);
        assert_eq!(record.access(), operation.access_kind());
        assert_eq!(record.context(), context);
        assert_eq!(record.cr2_value(), LINEAR);
    }

    fn assert_matrix_success(
        initial_entries: [u64; 4],
        paging: Long64ProbeConfig,
        context: AccessContext,
        access: AccessKind,
    ) {
        let (mut memory, _) = setup_entries(initial_entries, 0x6000);
        memory
            .write_u8(GuestPhysicalAddress::new(0x4abc), 0x5a, |_| {})
            .unwrap();
        let mut callbacks = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request_as(context, operation(access)),
            |range| callbacks.push(range),
        );
        let addresses = [0x1008, 0x2010, 0x3018, 0x4020];
        let a_callbacks = addresses.map(|address| (address, 8));
        match access {
            AccessKind::Read => {
                assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0x5a)));
                assert_eq!(report.ad_update_count, 4);
                assert_eq!(
                    report.ad_updates,
                    expected_matrix_updates(initial_entries, 4)
                );
                assert_eq!(logged(&callbacks), a_callbacks);
                assert_eq!(
                    entries(&memory),
                    initial_entries.map(|entry| entry | (1 << 5))
                );
            }
            AccessKind::Fetch => {
                assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Fetched(0x5a)));
                assert_eq!(report.ad_update_count, 4);
                assert_eq!(
                    report.ad_updates,
                    expected_matrix_updates(initial_entries, 4)
                );
                assert_eq!(logged(&callbacks), a_callbacks);
                assert_eq!(
                    entries(&memory),
                    initial_entries.map(|entry| entry | (1 << 5))
                );
            }
            AccessKind::Write => {
                assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
                assert_eq!(report.ad_update_count, 5);
                let mut expected = expected_matrix_updates(initial_entries, 4);
                expected[4] = Some(AdUpdate {
                    level: PageTableLevel::Pt,
                    address: GuestPhysicalAddress::new(0x4020),
                    before: initial_entries[3] | (1 << 5),
                    after: initial_entries[3] | (1 << 5) | (1 << 6),
                    bit: AdBit::Dirty,
                });
                assert_eq!(report.ad_updates, expected);
                assert_eq!(
                    logged(&callbacks),
                    [a_callbacks.as_slice(), &[(0x4020, 8), (0x4abc, 1)]].concat()
                );
                assert_eq!(
                    entries(&memory),
                    [
                        initial_entries[0] | (1 << 5),
                        initial_entries[1] | (1 << 5),
                        initial_entries[2] | (1 << 5),
                        initial_entries[3] | (1 << 5) | (1 << 6),
                    ]
                );
                assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x4abc)), Ok(0xa5));
            }
        }
    }

    #[test]
    fn tracked_nonpresent_matrix_covers_every_level_access_and_cpl() {
        let privileges = [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3];
        let accesses = [
            (AccessKind::Read, 0, 4),
            (AccessKind::Write, 2, 6),
            (AccessKind::Fetch, 0x10, 0x14),
        ];
        for index in 0..4 {
            for (access, supervisor_code, user_code) in accesses {
                for (cpl, error_code) in
                    [(privileges[0], supervisor_code), (privileges[1], user_code)]
                {
                    let mut entries = [0x2007, 0x3007, 0x4007, 0x4007];
                    entries[index] = 0;
                    assert_matrix_fault(
                        entries,
                        paging_config(false, true),
                        privilege(cpl),
                        operation(access),
                        index,
                        error_code,
                        PageFaultReason::NotPresent {
                            level: level(index),
                        },
                    );
                }
            }
        }
    }

    #[test]
    fn tracked_user_supervisor_matrix_accumulates_us_at_every_level() {
        for index in 0..4 {
            let entries = matrix_entries_with_bit(index, 1 << 2, false);
            for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
                for (privilege_level, error_code) in [
                    (
                        PrivilegeLevel::Ring3,
                        match access {
                            AccessKind::Read => 0x05,
                            AccessKind::Write => 0x07,
                            AccessKind::Fetch => 0x15,
                        },
                    ),
                    (PrivilegeLevel::Ring0, 0),
                ] {
                    if privilege_level == PrivilegeLevel::Ring3 {
                        assert_matrix_fault(
                            entries,
                            paging_config(false, true),
                            privilege(privilege_level),
                            operation(access),
                            4,
                            error_code,
                            PageFaultReason::Protection {
                                user_denied: true,
                                write_denied: false,
                                execute_denied: false,
                            },
                        );
                    } else {
                        assert_matrix_success(
                            entries,
                            paging_config(false, true),
                            privilege(privilege_level),
                            access,
                        );
                    }
                }
                for privilege_level in [PrivilegeLevel::Ring1, PrivilegeLevel::Ring2] {
                    assert_matrix_success(
                        entries,
                        paging_config(false, true),
                        privilege(privilege_level),
                        access,
                    );
                }
            }
        }
    }

    #[test]
    fn tracked_read_write_permissions_cover_every_level_and_wp_cpl_combination() {
        for index in 0..4 {
            let read_only = matrix_entries_with_bit(index, 1 << 1, false);
            for privilege_level in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
                for access in [AccessKind::Read, AccessKind::Fetch] {
                    assert_matrix_success(
                        read_only,
                        paging_config(false, true),
                        privilege(privilege_level),
                        access,
                    );
                }
            }

            assert_matrix_success(
                read_only,
                paging_config(false, true),
                privilege(PrivilegeLevel::Ring0),
                AccessKind::Write,
            );
            assert_matrix_fault(
                read_only,
                paging_config(true, true),
                privilege(PrivilegeLevel::Ring0),
                operation(AccessKind::Write),
                4,
                0x03,
                PageFaultReason::Protection {
                    user_denied: false,
                    write_denied: true,
                    execute_denied: false,
                },
            );
            for wp in [false, true] {
                assert_matrix_fault(
                    read_only,
                    paging_config(wp, true),
                    privilege(PrivilegeLevel::Ring3),
                    operation(AccessKind::Write),
                    4,
                    0x07,
                    PageFaultReason::Protection {
                        user_denied: false,
                        write_denied: true,
                        execute_denied: false,
                    },
                );
            }
            for privilege_level in [PrivilegeLevel::Ring1, PrivilegeLevel::Ring2] {
                assert_matrix_success(
                    read_only,
                    paging_config(false, true),
                    privilege(privilege_level),
                    AccessKind::Write,
                );
                assert_matrix_fault(
                    read_only,
                    paging_config(true, true),
                    privilege(privilege_level),
                    operation(AccessKind::Write),
                    4,
                    0x03,
                    PageFaultReason::Protection {
                        user_denied: false,
                        write_denied: true,
                        execute_denied: false,
                    },
                );
            }
        }
    }

    #[test]
    fn wp_read_then_write_reuses_same_memory_and_preserves_preset_accessed_state() {
        let initial_entries = [0x2005, 0x3005, 0x4005, 0x4005];
        let (mut memory, _) = setup_entries(initial_entries, 0x6000);
        let paging = paging_config(true, true);
        let context = privilege(PrivilegeLevel::Ring0);
        memory
            .write_u8(GuestPhysicalAddress::new(0x4abc), 0x5a, |_| {})
            .unwrap();

        let mut read_callbacks = Vec::new();
        let read = access_long64_4k_ram(
            &mut memory,
            &paging,
            request_as(
                context,
                RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                },
            ),
            |range| read_callbacks.push(range),
        );
        assert_eq!(read.outcome, Ok(TrackedAccessOutcome::Read(0x5a)));
        assert_eq!(read.ad_update_count, 4);
        assert_eq!(read.ad_updates, expected_matrix_updates(initial_entries, 4));
        assert_eq!(
            logged(&read_callbacks),
            [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
        );
        let after_read = initial_entries.map(|entry| entry | (1 << 5));
        assert_eq!(entries(&memory), after_read);

        let operand_before = ram_snapshot(&memory, 0x4abc, 8);
        let mut write_callbacks = Vec::new();
        let write = access_long64_4k_ram(
            &mut memory,
            &paging,
            request_as(
                context,
                RamAccessOperation::Write {
                    width: AccessWidth::Byte,
                    value: 0xa5,
                },
            ),
            |range| write_callbacks.push(range),
        );
        let expected_fault = PageFaultRecord::new(RIP, AccessKind::Write, context, 0x03, LINEAR);
        assert_eq!(
            &write.outcome,
            &Err(TrackedAccessError::PageFault {
                record: expected_fault,
                reason: PageFaultReason::Protection {
                    user_denied: false,
                    write_denied: true,
                    execute_denied: false,
                },
            })
        );
        assert_eq!(write.trace.len(), 4);
        assert_eq!(write.ad_update_count, 0);
        assert_eq!(write.ad_updates, [None; 5]);
        assert!(write_callbacks.is_empty());
        assert_eq!(entries(&memory), after_read);
        assert_eq!(ram_snapshot(&memory, 0x4abc, 8), operand_before);
        let record = match write.outcome.as_ref().unwrap_err() {
            TrackedAccessError::PageFault { record, .. } => record,
            other => panic!("expected page fault, got {other:?}"),
        };
        assert_eq!(record.fault().vector, 14);
        assert_eq!(record.fault().error_code, Some(0x03));
        assert_eq!(record.fault().linear_address, Some(LINEAR));
        assert_eq!(record.saved_rip(), RIP);
        assert_eq!(record.access(), AccessKind::Write);
        assert_eq!(record.context(), context);
        assert_eq!(record.cr2_value(), LINEAR);
    }

    #[test]
    fn tracked_nx_matrix_checks_fetch_faults_reserved_bits_and_data_access() {
        for index in 0..4 {
            let nx_entries = matrix_entries_with_bit(index, ENTRY_NX_BIT, true);
            for (privilege_level, error_code) in
                [(PrivilegeLevel::Ring0, 0x11), (PrivilegeLevel::Ring3, 0x15)]
            {
                assert_matrix_fault(
                    nx_entries,
                    paging_config(false, true),
                    privilege(privilege_level),
                    operation(AccessKind::Fetch),
                    4,
                    error_code,
                    PageFaultReason::Protection {
                        user_denied: false,
                        write_denied: false,
                        execute_denied: true,
                    },
                );
                for access in [AccessKind::Read, AccessKind::Write] {
                    assert_matrix_success(
                        nx_entries,
                        paging_config(false, true),
                        privilege(privilege_level),
                        access,
                    );
                }
            }
            for privilege_level in [PrivilegeLevel::Ring1, PrivilegeLevel::Ring2] {
                assert_matrix_fault(
                    nx_entries,
                    paging_config(false, true),
                    privilege(privilege_level),
                    operation(AccessKind::Fetch),
                    4,
                    0x11,
                    PageFaultReason::Protection {
                        user_denied: false,
                        write_denied: false,
                        execute_denied: true,
                    },
                );
            }

            for (privilege_level, error_code) in
                [(PrivilegeLevel::Ring0, 0x09), (PrivilegeLevel::Ring3, 0x0d)]
            {
                assert_matrix_fault(
                    nx_entries,
                    paging_config(false, false),
                    privilege(privilege_level),
                    operation(AccessKind::Fetch),
                    index,
                    error_code,
                    PageFaultReason::ReservedBits {
                        level: level(index),
                        offending_bits: ENTRY_NX_BIT,
                    },
                );
            }
        }
    }

    #[test]
    fn pure_probe_permission_cases_never_commit_accessed_or_dirty_bits() {
        let initial_entries = [0x2007, 0x3007, 0x4007, 0x4007];
        let (memory, paging) = setup_entries(initial_entries, 0x6000);
        for privilege_level in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
            for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
                assert!(matches!(
                    probe_long64_4k(
                        &memory,
                        &paging,
                        GuestLinearAddress::new(LINEAR),
                        access,
                        privilege(privilege_level),
                    )
                    .outcome,
                    Ok(ProbeOutcome::Translated(_))
                ));
                assert_eq!(entries(&memory), initial_entries);
            }
        }
    }

    #[test]
    fn preset_accessed_and_dirty_bits_do_not_emit_redundant_table_callbacks() {
        let entries_before = [0x2027, 0x3027, 0x4027, 0x4067];
        let (mut memory, paging) = setup_entries(entries_before, 0x6000);
        let mut callbacks = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xa5,
            }),
            |range| callbacks.push(range),
        );
        assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
        assert_eq!(report.ad_update_count, 0);
        assert_eq!(report.ad_updates, [None; 5]);
        assert_eq!(entries(&memory), entries_before);
        assert_eq!(logged(&callbacks), [(0x4abc, 1)]);
        assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x4abc)), Ok(0xa5));
    }

    #[test]
    fn large_leaf_offsets_dirty_and_legacy_admission() {
        for offset in [0, 0x10000, 0x1fffff] {
            let (mut memory, paging) = setup(0, 0x400000);
            memory
                .write_u64(GuestPhysicalAddress::new(0x3018), 0x201087, |_| {})
                .unwrap();
            let physical = 0x200000 + offset;
            memory
                .write_u8(GuestPhysicalAddress::new(physical), 0x5a, |_| {})
                .unwrap();
            let mut req = request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            });
            req.address = GuestLinearAddress::new((LINEAR & !0x1fffff) | offset);
            let legacy = access_long64_4k_ram(&mut memory, &paging, req, |_| {});
            assert!(matches!(
                legacy.outcome,
                Err(TrackedAccessError::UnsupportedPageSize {
                    level: PageTableLevel::Pd,
                    ..
                })
            ));
            let report = access_long64_2m_ram(&mut memory, &paging, req, |_| {});
            assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0x5a)));
            assert_eq!(report.trace.len(), 3);
            req.operation = RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xa5,
            };
            let mut log = Vec::new();
            let write = access_long64_2m_ram(&mut memory, &paging, req, |range| log.push(range));
            assert_eq!(write.outcome, Ok(TrackedAccessOutcome::Written));
            assert_eq!(write.ad_updates[0].unwrap().level, PageTableLevel::Pd);
            assert_eq!(write.ad_updates[0].unwrap().bit, AdBit::Dirty);
            assert_eq!(logged(&log), [(0x3018, 8), (physical, 1)]);
        }
    }

    #[test]
    fn large_leaf_reserved_bits_fault_keeps_ancestor_accessed_prefix() {
        let (mut memory, paging) = setup(0, 0x400000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x3018), 0x202087, |_| {})
            .unwrap();
        let mut log = Vec::new();
        let report = access_long64_2m_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |range| log.push(range),
        );
        let TrackedAccessError::PageFault { record, reason } = report.outcome.unwrap_err() else {
            panic!("expected fault")
        };
        assert_eq!(record.saved_rip(), RIP);
        assert_eq!(record.cr2_value(), LINEAR);
        assert_eq!(record.fault().error_code, Some(9));
        assert!(matches!(
            reason,
            PageFaultReason::ReservedBits {
                level: PageTableLevel::Pd,
                offending_bits: 0x2000
            }
        ));
        assert_eq!(logged(&log), [(0x1008, 8), (0x2010, 8)]);
    }

    #[test]
    fn large_leaf_permissions_and_table_write_failures() {
        for (pde, wp, kind, expected) in [
            (0x200085, true, AccessKind::Write, Some(3)),
            (0x200085, false, AccessKind::Write, None),
            (0x8000_0000_0020_0087, true, AccessKind::Fetch, Some(17)),
        ] {
            let (mut memory, _) = setup(0, 0x400000);
            memory
                .write_u64(GuestPhysicalAddress::new(0x3018), pde, |_| {})
                .unwrap();
            let paging = Long64ProbeConfig::new(
                0x8000_0033 | if wp { CR0_WP } else { 0 },
                0x1000,
                CR4_PAE,
                EFER_LME | EFER_LMA | EFER_NXE,
            )
            .unwrap();
            let operation = if kind == AccessKind::Fetch {
                RamAccessOperation::Fetch
            } else {
                RamAccessOperation::Write {
                    width: AccessWidth::Byte,
                    value: 42,
                }
            };
            let result = access_long64_2m_ram(&mut memory, &paging, request(operation), |_| {});
            match expected {
                Some(code) => match result.outcome {
                    Err(TrackedAccessError::PageFault { record, .. }) => {
                        assert_eq!(record.fault().error_code, Some(code))
                    }
                    other => panic!("unexpected {other:?}"),
                },
                None => assert_eq!(result.outcome, Ok(TrackedAccessOutcome::Written)),
            }
        }

        // A table-entry write failure at the PDE reports the PD level for both
        // the accessed bit (walk) and the dirty bit (leaf update).
        for (pde, bit, operation) in [
            (
                0x200087,
                AdBit::Accessed,
                RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                },
            ),
            (
                0x2000a7,
                AdBit::Dirty,
                RamAccessOperation::Write {
                    width: AccessWidth::Byte,
                    value: 1,
                },
            ),
        ] {
            let (mut memory, paging) = setup(0, 0x400000);
            let pde_address = GuestPhysicalAddress::new(0x3018);
            memory.write_u64(pde_address, pde, |_| {}).unwrap();
            let mut log = Vec::new();
            let mut callback = |range| log.push(range);
            let mut writer =
                |memory: &mut PhysicalMemory,
                 address: GuestPhysicalAddress,
                 value: u64,
                 callback: &mut dyn FnMut(GuestPhysicalRange)| {
                    if address == pde_address {
                        Err(MemoryAccessError::Unmapped(address))
                    } else {
                        write_tracked_table_entry(memory, address, value, callback)
                    }
                };
            let report = access_long64_4k_internal_with_writer(
                &mut memory,
                &paging,
                request(operation),
                &mut callback,
                false,
                None,
                &mut writer,
                true,
            );
            assert_eq!(
                report.outcome,
                Err(TrackedAccessError::TableWrite {
                    level: PageTableLevel::Pd,
                    entry_address: pde_address,
                    bit,
                    source: MemoryAccessError::Unmapped(pde_address),
                })
            );
            assert_eq!(logged(&log), [(0x1008, 8), (0x2010, 8)]);
        }
    }

    #[test]
    fn large_leaf_aggregates_user_write_and_nx_rights_from_ancestors() {
        let wp_paging = || {
            Long64ProbeConfig::new(
                0x8000_0033 | CR0_WP,
                0x1000,
                CR4_PAE,
                EFER_LME | EFER_LMA | EFER_NXE,
            )
            .unwrap()
        };
        let ring3 = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring3,
            origin: AccessOrigin::Explicit,
        };
        let read = RamAccessOperation::Read {
            width: AccessWidth::Byte,
        };
        let write = RamAccessOperation::Write {
            width: AccessWidth::Byte,
            value: 1,
        };
        // (entries, request, expected PFEC): the 2 MiB PDE alone would allow
        // each access; the denying bit comes from the PDE (U/S) or an ancestor.
        let cases = [
            // Ring3 read through a supervisor-only PDE: P|U.
            ([0x2007, 0x3007, 0x200083, 0], request_as(ring3, read), 5),
            // Supervisor write, WP=1, read-only PDPT entry above a writable PDE: P|W.
            ([0x2007, 0x3005, 0x200087, 0], request(write), 3),
            // NX on the PML4 entry, instruction fetch: P|I.
            (
                [0x8000_0000_0000_2007, 0x3007, 0x200087, 0],
                request(RamAccessOperation::Fetch),
                17,
            ),
        ];
        for (entries, req, expected) in cases {
            let (mut memory, _) = setup_entries(entries, 0x400000);
            let report = access_long64_2m_ram(&mut memory, &wp_paging(), req, |_| {});
            let Err(TrackedAccessError::PageFault { record, reason }) = report.outcome else {
                panic!("expected protection fault for {entries:x?}")
            };
            assert_eq!(record.fault().error_code, Some(expected), "{entries:x?}");
            assert!(matches!(reason, PageFaultReason::Protection { .. }));
            // A bits were committed on every level walked before the denial.
            assert_eq!(report.ad_update_count, 3);
        }
    }

    #[test]
    fn large_leaf_reserved_mask_covers_bit_20_and_write_pfec() {
        let (mut memory, paging) = setup(0, 0x400000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x3018), 0x300087, |_| {})
            .unwrap();
        let report = access_long64_2m_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 1,
            }),
            |_| {},
        );
        let Err(TrackedAccessError::PageFault { record, reason }) = report.outcome else {
            panic!("expected fault")
        };
        // RSVD|W|P for a write.
        assert_eq!(record.fault().error_code, Some(0xb));
        assert!(matches!(
            reason,
            PageFaultReason::ReservedBits {
                level: PageTableLevel::Pd,
                offending_bits: 0x100000
            }
        ));
    }

    #[test]
    fn first_write_through_fresh_large_leaf_sets_a_then_d_before_operand() {
        let (mut memory, paging) = setup(0, 0x400000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x3018), 0x200087, |_| {})
            .unwrap();
        let mut log = Vec::new();
        let report = access_long64_2m_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xa5,
            }),
            |range| log.push(range),
        );
        assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
        assert_eq!(
            logged(&log),
            [
                (0x1008, 8),
                (0x2010, 8),
                (0x3018, 8),
                (0x3018, 8),
                (0x200000 + (LINEAR & 0x1fffff), 1)
            ]
        );
        let bits = report.ad_updates[..report.ad_update_count]
            .iter()
            .map(|update| {
                let update = update.unwrap();
                (update.level, update.bit)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            bits,
            [
                (PageTableLevel::Pml4, AdBit::Accessed),
                (PageTableLevel::Pdpt, AdBit::Accessed),
                (PageTableLevel::Pd, AdBit::Accessed),
                (PageTableLevel::Pd, AdBit::Dirty),
            ]
        );
        assert_eq!(entries(&memory)[2], 0x2000e7);
    }

    #[test]
    fn large_leaf_does_not_admit_pdpt_ps_and_rejects_mmio_operand() {
        // S4-10c: заменяет host-ошибку S4-10a. PS on a PDPT entry is not
        // admitted even in the 2 MiB profile: guest #PF P|RSVD (PFEC 0x09), A set
        // only on the PML4E, none on the PDPTE.
        let (mut memory, paging) = setup_entries([0x2007, 0x3087, 0x4007, 0], 0x400000);
        let report = access_long64_2m_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |_| {},
        );
        assert_eq!(
            report.outcome,
            ps_tracked_expected(
                PageTableLevel::Pdpt,
                0x2010,
                0x3087,
                RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                },
                true,
                0
            )
        );
        assert_eq!(report.trace.len(), 2);
        assert_eq!(report.ad_update_count, 1);
        assert_eq!(entries(&memory)[0], 0x2027);
        assert_eq!(entries(&memory)[1], 0x3087);

        // The profile is RAM-only: a 2 MiB leaf mapped beyond RAM is an
        // operand-physical error, never a device dispatch.
        let (mut memory, paging) = setup(0, 0x400000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x3018), 0x4000_0087, |_| {})
            .unwrap();
        let report = access_long64_2m_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |_| {},
        );
        assert!(matches!(
            report.outcome,
            Err(TrackedAccessError::OperandPhysical { .. })
        ));
    }

    #[test]
    fn bounded_cross_page_read_accepts_framebuffer_data_and_table_in_framebuffer_is_refused() {
        let fb_start = 0x10_000u64;
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x6000).unwrap(),
                    MemoryRegionKind::Ram,
                ),
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(fb_start), 0x4000).unwrap(),
                    MemoryRegionKind::Framebuffer,
                ),
            ],
        )
        .unwrap();
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x6000)).unwrap();
        for (address, value) in [
            (0x1008, 0x2007),
            (0x2010, 0x3007),
            (0x3018, 0x4007),
            (0x4020, fb_start | 7),
            (0x4028, (fb_start + 0x1000) | 7),
        ] {
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        memory
            .write_bytes(
                GuestPhysicalAddress::new(fb_start + 0xffe),
                &[1, 2, 3, 4],
                |_| panic!("framebuffer writes never notify"),
            )
            .unwrap();
        let paging =
            Long64ProbeConfig::new(0x8000_0033, 0x1000, CR4_PAE, EFER_LME | EFER_LMA | EFER_NXE)
                .unwrap();
        let mut request = request(RamAccessOperation::Read {
            width: AccessWidth::Dword,
        });
        request.address = GuestLinearAddress::new(0x0000_0080_8060_4ffe);
        let value = read_long64_4k_bounded_ram_read(&mut memory, &paging, request, |_| {});
        assert_eq!(value, Ok(0x0403_0201));

        // The same walk with the root table inside the framebuffer region.
        let tables_in_fb = Long64ProbeConfig::new(
            0x8000_0033,
            fb_start,
            CR4_PAE,
            EFER_LME | EFER_LMA | EFER_NXE,
        )
        .unwrap();
        assert!(matches!(
            read_long64_4k_bounded_ram_read(&mut memory, &tables_in_fb, request, |_| {}),
            Err(TrackedAccessError::TableRead {
                source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Framebuffer),
                ..
            })
        ));
        let probe = probe_long64_4k(
            &memory,
            &tables_in_fb,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            request.context,
        );
        assert!(matches!(
            probe.outcome,
            Err(ProbeError::Access(ProbeAccessError::TableRead {
                source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Framebuffer),
                ..
            }))
        ));
    }

    #[test]
    fn successful_read_and_fetch_commit_only_accessed_bits() {
        for operation in [
            RamAccessOperation::Read {
                width: AccessWidth::Dword,
            },
            RamAccessOperation::Fetch,
        ] {
            let (mut memory, paging) = setup(0x4007, 0x6000);
            memory
                .write_u32(GuestPhysicalAddress::new(0x4abc), 0x7856_3412, |_| {})
                .unwrap();
            let mut log = Vec::new();
            let report = access_long64_4k_ram(&mut memory, &paging, request(operation), |range| {
                log.push(range)
            });
            assert!(matches!(
                report.outcome,
                Ok(TrackedAccessOutcome::Read(0x7856_3412))
                    | Ok(TrackedAccessOutcome::Fetched(0x12))
            ));
            assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x4027]);
            assert_eq!(report.ad_update_count, 4);
            assert_eq!(
                logged(&log),
                [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
            );
        }
    }

    #[test]
    fn accessed_write_failures_preserve_committed_prefix_at_each_level() {
        let levels = [
            PageTableLevel::Pml4,
            PageTableLevel::Pdpt,
            PageTableLevel::Pd,
            PageTableLevel::Pt,
        ];
        let addresses = [0x1008, 0x2010, 0x3018, 0x4020];
        let original = [0x2007, 0x3007, 0x4007, 0x4007];

        for failed_index in 0..levels.len() {
            let (mut memory, paging) = setup(0x4007, 0x6000);
            let failed_address = GuestPhysicalAddress::new(addresses[failed_index]);
            let mut attempts = Vec::new();
            let mut callbacks = Vec::new();
            let mut writes =
                |memory: &mut PhysicalMemory,
                 address: GuestPhysicalAddress,
                 value: u64,
                 callback: &mut dyn FnMut(GuestPhysicalRange)| {
                    attempts.push(address);
                    if address == failed_address {
                        Err(MemoryAccessError::Unmapped(address))
                    } else {
                        write_tracked_table_entry(memory, address, value, callback)
                    }
                };
            let mut callback = |range| callbacks.push(range);
            let report = access_long64_4k_internal_with_writer(
                &mut memory,
                &paging,
                request(RamAccessOperation::Write {
                    width: AccessWidth::Byte,
                    value: 0xa5,
                }),
                &mut callback,
                false,
                None,
                &mut writes,
                false,
            );

            assert!(matches!(
                report.outcome,
                Err(TrackedAccessError::TableWrite {
                    level,
                    entry_address,
                    bit: AdBit::Accessed,
                    source: MemoryAccessError::Unmapped(source_address),
                }) if level == levels[failed_index]
                    && entry_address == failed_address
                    && source_address == failed_address
            ));
            assert_eq!(report.ad_update_count, failed_index);
            assert_eq!(report.ad_updates, expected_accessed_updates(failed_index));
            assert_eq!(
                attempts,
                addresses[..=failed_index]
                    .iter()
                    .copied()
                    .map(GuestPhysicalAddress::new)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                logged(&callbacks),
                addresses[..failed_index]
                    .iter()
                    .map(|address| (*address, 8))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                entries(&memory),
                std::array::from_fn(|index| {
                    original[index] | if index < failed_index { 1 << 5 } else { 0 }
                })
            );
            assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x4abc)), Ok(0));
            assert_eq!(memory.read_u64(failed_address), Ok(original[failed_index]));
        }
    }

    #[test]
    fn fetch_accessed_write_failures_are_tracked_before_physical_fetch() {
        let levels = [
            PageTableLevel::Pml4,
            PageTableLevel::Pdpt,
            PageTableLevel::Pd,
            PageTableLevel::Pt,
        ];
        let addresses = [0x1008, 0x2010, 0x3018, 0x4020];
        let original = [0x2007, 0x3007, 0x4007, 0x4007];
        let fetch = request_as(privilege(PrivilegeLevel::Ring3), RamAccessOperation::Fetch);
        for failed_index in 0..levels.len() {
            let (mut memory, paging) = setup(0x4007, 0x6000);
            let failed_address = GuestPhysicalAddress::new(addresses[failed_index]);
            let mut callbacks = Vec::new();
            let mut attempts = Vec::new();
            let mut writer =
                |memory: &mut PhysicalMemory,
                 address: GuestPhysicalAddress,
                 value: u64,
                 callback: &mut dyn FnMut(GuestPhysicalRange)| {
                    attempts.push(address);
                    if address == failed_address {
                        Err(MemoryAccessError::Unmapped(address))
                    } else {
                        write_tracked_table_entry(memory, address, value, callback)
                    }
                };
            let mut callback = |range| callbacks.push(range);
            let report = access_long64_4k_internal_with_writer(
                &mut memory,
                &paging,
                fetch,
                &mut callback,
                false,
                None,
                &mut writer,
                false,
            );
            assert!(
                matches!(report.outcome, Err(TrackedAccessError::TableWrite { level, entry_address, bit: AdBit::Accessed, source: MemoryAccessError::Unmapped(source_address) }) if level == levels[failed_index] && entry_address == failed_address && source_address == failed_address)
            );
            assert_eq!(report.ad_update_count, failed_index);
            assert_eq!(report.ad_updates, expected_accessed_updates(failed_index));
            assert_eq!(
                attempts,
                addresses[..=failed_index]
                    .iter()
                    .copied()
                    .map(GuestPhysicalAddress::new)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                logged(&callbacks),
                addresses[..failed_index]
                    .iter()
                    .map(|address| (*address, 8))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                entries(&memory),
                std::array::from_fn(
                    |index| original[index] | if index < failed_index { 1 << 5 } else { 0 }
                )
            );
            assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x4abc)), Ok(0));
        }
    }

    #[test]
    fn dirty_write_failure_preserves_accessed_prefix_without_operand_commit() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        let dirty_address = GuestPhysicalAddress::new(0x4020);
        let mut attempts = Vec::new();
        let mut callbacks = Vec::new();
        let mut pt_writes = 0;
        let mut writes = |memory: &mut PhysicalMemory,
                          address: GuestPhysicalAddress,
                          value: u64,
                          callback: &mut dyn FnMut(GuestPhysicalRange)| {
            attempts.push(address);
            if address == dirty_address {
                pt_writes += 1;
                if pt_writes == 2 {
                    return Err(MemoryAccessError::Unmapped(address));
                }
            }
            write_tracked_table_entry(memory, address, value, callback)
        };
        let mut callback = |range| callbacks.push(range);
        let report = access_long64_4k_internal_with_writer(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xa5,
            }),
            &mut callback,
            false,
            None,
            &mut writes,
            false,
        );

        assert_eq!(
            report.outcome,
            Err(TrackedAccessError::TableWrite {
                level: PageTableLevel::Pt,
                entry_address: dirty_address,
                bit: AdBit::Dirty,
                source: MemoryAccessError::Unmapped(dirty_address),
            })
        );
        assert_eq!(report.ad_update_count, 4);
        assert_eq!(report.ad_updates, expected_accessed_updates(4));
        assert_eq!(
            attempts,
            [0x1008, 0x2010, 0x3018, 0x4020, 0x4020].map(GuestPhysicalAddress::new)
        );
        assert_eq!(
            logged(&callbacks),
            [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
        );
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x4027]);
        assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x4abc)), Ok(0));
    }

    #[test]
    fn write_commits_accessed_dirty_then_operand_in_order() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        let mut log = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Word,
                value: 0xbeef,
            }),
            |range| log.push(range),
        );
        assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x4067]);
        assert_eq!(
            memory.read_u16(GuestPhysicalAddress::new(0x4abc)).unwrap(),
            0xbeef
        );
        assert_eq!(report.ad_update_count, 5);
        assert_eq!(
            report.ad_updates,
            [
                Some(AdUpdate {
                    level: PageTableLevel::Pml4,
                    address: GuestPhysicalAddress::new(0x1008),
                    before: 0x2007,
                    after: 0x2027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pdpt,
                    address: GuestPhysicalAddress::new(0x2010),
                    before: 0x3007,
                    after: 0x3027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pd,
                    address: GuestPhysicalAddress::new(0x3018),
                    before: 0x4007,
                    after: 0x4027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pt,
                    address: GuestPhysicalAddress::new(0x4020),
                    before: 0x4007,
                    after: 0x4027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pt,
                    address: GuestPhysicalAddress::new(0x4020),
                    before: 0x4027,
                    after: 0x4067,
                    bit: AdBit::Dirty,
                }),
            ]
        );
        assert_eq!(
            logged(&log),
            [
                (0x1008, 8),
                (0x2010, 8),
                (0x3018, 8),
                (0x4020, 8),
                (0x4020, 8),
                (0x4abc, 2)
            ]
        );
    }

    #[test]
    fn pre_set_accessed_and_dirty_bits_do_not_create_extra_commits() {
        let (mut memory, paging) = setup(0x4067, 0x6000);
        for (address, value) in [(0x1008, 0x2027), (0x2010, 0x3027), (0x3018, 0x4027)] {
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        let mut log = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xaa,
            }),
            |range| log.push(range),
        );
        assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
        assert_eq!(report.ad_update_count, 0);
        assert_eq!(logged(&log), [(0x4abc, 1)]);
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x4067]);
    }

    #[test]
    fn nonpresent_reserved_and_protection_faults_keep_prior_accessed_prefix() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x3018), 0, |_| {})
            .unwrap();
        let mut log = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |range| log.push(range),
        );
        let TrackedAccessError::PageFault { record, reason } = report.outcome.unwrap_err() else {
            panic!("expected #PF record")
        };
        assert_eq!(record.fault().vector, 14);
        assert_eq!(record.fault().error_code, Some(0));
        assert_eq!(record.saved_rip(), RIP);
        assert_eq!(record.cr2_value(), LINEAR);
        assert_eq!(
            reason,
            PageFaultReason::NotPresent {
                level: PageTableLevel::Pd
            }
        );
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0, 0x4007]);
        assert_eq!(logged(&log), [(0x1008, 8), (0x2010, 8)]);

        let (mut memory, paging) = setup(0x4007 | (1 << 36), 0x6000);
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |_| {},
        );
        assert!(matches!(
            report.outcome,
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::ReservedBits {
                    level: PageTableLevel::Pt,
                    ..
                },
                ..
            })
        ));
        assert_eq!(
            entries(&memory),
            [0x2027, 0x3027, 0x4027, 0x4007 | (1 << 36)]
        );

        let (mut memory, _) = setup(0x4005, 0x6000);
        let paging = Long64ProbeConfig::new(
            0x8000_0033 | CR0_WP,
            0x1000,
            CR4_PAE,
            EFER_LME | EFER_LMA | EFER_NXE,
        )
        .unwrap();
        let mut write_request = request(RamAccessOperation::Write {
            width: AccessWidth::Byte,
            value: 1,
        });
        write_request.context.privilege = PrivilegeLevel::Ring3;
        let report = access_long64_4k_ram(&mut memory, &paging, write_request, |_| {});
        assert!(matches!(
            report.outcome,
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::Protection {
                    write_denied: true,
                    ..
                },
                ..
            })
        ));
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x4025]);
    }

    #[test]
    fn table_read_and_operand_failures_are_physical_errors_with_committed_prefix() {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x3000).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut short = PhysicalMemory::new(&map, RamLimits::new(0x3000)).unwrap();
        short
            .write_u64(GuestPhysicalAddress::new(0x1008), 0x2007, |_| {})
            .unwrap();
        short
            .write_u64(GuestPhysicalAddress::new(0x2010), 0x3007, |_| {})
            .unwrap();
        let paging =
            Long64ProbeConfig::new(0x8000_0033, 0x1000, CR4_PAE, EFER_LME | EFER_LMA | EFER_NXE)
                .unwrap();
        let mut log = Vec::new();
        let report = access_long64_4k_ram(
            &mut short,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |range| log.push(range),
        );
        assert!(
            matches!(report.outcome, Err(TrackedAccessError::TableRead { level: PageTableLevel::Pd, entry_address, .. }) if entry_address.get() == 0x3018)
        );
        assert_eq!(logged(&log), [(0x1008, 8), (0x2010, 8)]);

        let (mut memory, paging) = setup(0x9007, 0x6000);
        let mut log = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0x55,
            }),
            |range| log.push(range),
        );
        assert!(
            matches!(report.outcome, Err(TrackedAccessError::OperandPhysical { address, .. }) if address.get() == 0x9abc)
        );
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x9027]);
        assert_eq!(
            logged(&log),
            [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
        );
    }

    #[test]
    fn writes_to_rom_and_reserved_are_write_errors_after_a_without_dirty_or_operand_commit() {
        for kind in [MemoryRegionKind::Rom, MemoryRegionKind::Reserved] {
            let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x6000).unwrap();
            let target =
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0x9000), 0x1000).unwrap();
            let map = MachineMemoryMap::new(
                36,
                vec![
                    MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
                    MemoryRegionSpec::new(target, kind),
                ],
            )
            .unwrap();
            let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x6000)).unwrap();
            for (address, value) in [
                (0x1008, 0x2007),
                (0x2010, 0x3007),
                (0x3018, 0x4007),
                (0x4020, 0x9007),
            ] {
                memory
                    .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                    .unwrap();
            }
            let paging = Long64ProbeConfig::new(
                0x8000_0033,
                0x1000,
                CR4_PAE,
                EFER_LME | EFER_LMA | EFER_NXE,
            )
            .unwrap();
            let mut callback_ranges = Vec::new();
            let report = access_long64_4k_ram(
                &mut memory,
                &paging,
                request(RamAccessOperation::Write {
                    width: AccessWidth::Dword,
                    value: 0xfeed_face,
                }),
                |range| callback_ranges.push(range),
            );
            assert!(matches!(
                report.outcome,
                Err(TrackedAccessError::OperandPhysical {
                    address,
                    source: MemoryAccessError::ReadOnlyRegion(region_kind),
                }) if address.get() == 0x9abc && region_kind == kind
            ));
            assert_eq!(report.ad_update_count, 4);
            assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x9027]);
            assert_eq!(
                logged(&callback_ranges),
                [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
            );
            assert!(
                !callback_ranges
                    .iter()
                    .any(|range| range.start().get() == 0x9abc)
            );

            let read = access_long64_4k_ram(
                &mut memory,
                &paging,
                request(RamAccessOperation::Read {
                    width: AccessWidth::Dword,
                }),
                |_| {},
            );
            assert!(matches!(
                read.outcome,
                Err(TrackedAccessError::OperandPhysical {
                    source: MemoryAccessError::UnreadableRegion(region_kind),
                    ..
                }) if region_kind == kind
            ));
        }
    }

    #[test]
    fn mmio_write_is_device_unavailable_without_dispatch_or_operand_notification() {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x6000).unwrap();
        let mmio = GuestPhysicalRange::new(GuestPhysicalAddress::new(0x9000), 0x1000).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
                MemoryRegionSpec::new(mmio, MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x6000)).unwrap();
        for (address, value) in [
            (0x1008, 0x2007),
            (0x2010, 0x3007),
            (0x3018, 0x4007),
            (0x4020, 0x9007),
        ] {
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        let paging =
            Long64ProbeConfig::new(0x8000_0033, 0x1000, CR4_PAE, EFER_LME | EFER_LMA | EFER_NXE)
                .unwrap();
        let mut callback_ranges = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0xff,
            }),
            |range| callback_ranges.push(range),
        );
        assert_eq!(
            report.outcome,
            Err(TrackedAccessError::DeviceUnavailable {
                address: GuestPhysicalAddress::new(0x9abc),
            })
        );
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x9027]);
        assert_eq!(
            logged(&callback_ranges),
            [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
        );
    }

    #[test]
    fn cross_page_rejects_before_any_walk_or_commit_and_probe_stays_pure() {
        let (mut memory, paging) = setup(0x4007 | (1 << 5) | (1 << 6), 0x6000);
        let before = entries(&memory);
        let mut cross = request(RamAccessOperation::Write {
            width: AccessWidth::Dword,
            value: 1,
        });
        cross.address = GuestLinearAddress::new(0x0000_0080_8060_5ffe);
        let mut log = Vec::new();
        let report = access_long64_4k_ram(&mut memory, &paging, cross, |range| log.push(range));
        assert!(matches!(
            report.outcome,
            Err(TrackedAccessError::CrossesPage { .. })
        ));
        assert!(report.trace.is_empty());
        assert!(log.is_empty());
        assert_eq!(entries(&memory), before);

        let pure = probe_long64_4k(
            &memory,
            &paging,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Write,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            })
            .context,
        );
        assert!(matches!(pure.outcome, Ok(ProbeOutcome::Translated(_))));
        assert_eq!(entries(&memory), before);
    }

    #[test]
    fn same_page_read_write_widths_cover_last_fitting_offset_and_final_byte() {
        for (width, bytes, expected_read) in [
            (AccessWidth::Byte, 1u8, 0x10),
            (AccessWidth::Word, 2, 0x2110),
            (AccessWidth::Dword, 4, 0x4332_2110),
            (AccessWidth::Qword, 8, 0x8776_6554_4332_2110),
        ] {
            let (mut memory, paging) = setup(0x4007, 0x6000);
            let base = LINEAR & !0xfff;
            let offset = 0x1000 - u64::from(bytes);
            let address = GuestLinearAddress::new(base | offset);
            let physical = GuestPhysicalAddress::new(0x4000 + offset);
            let source = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];
            let written = [0xa0, 0xb1, 0xc2, 0xd3, 0xe4, 0xf5, 0x16, 0x27];
            memory
                .write_u8(GuestPhysicalAddress::new(physical.get() - 1), 0x5a, |_| {})
                .unwrap();
            memory.write_bytes(physical, &source, |_| {}).unwrap();
            memory
                .write_u8(GuestPhysicalAddress::new(physical.get() + 8), 0xa5, |_| {})
                .unwrap();
            let mut read_request = request(RamAccessOperation::Read { width });
            read_request.address = address;
            let read = access_long64_4k_ram(&mut memory, &paging, read_request, |_| {});
            assert_eq!(read.outcome, Ok(TrackedAccessOutcome::Read(expected_read)));

            let mut write_request = request(RamAccessOperation::Write {
                width,
                value: u64::from_le_bytes(written),
            });
            write_request.address = address;
            let write = access_long64_4k_ram(&mut memory, &paging, write_request, |_| {});
            assert_eq!(write.outcome, Ok(TrackedAccessOutcome::Written));
            assert_eq!(write.ad_update_count, 1);
            assert_eq!(write.trace.len(), 4);
            let mut after = [0; 8];
            memory.read_bytes(physical, &mut after).unwrap();
            assert_eq!(&after[..usize::from(bytes)], &written[..usize::from(bytes)]);
            assert_eq!(&after[usize::from(bytes)..], &source[usize::from(bytes)..]);
            assert_eq!(
                memory.read_u8(GuestPhysicalAddress::new(physical.get() - 1)),
                Ok(0x5a)
            );
            assert_eq!(
                memory.read_u8(GuestPhysicalAddress::new(physical.get() + 8)),
                Ok(0xa5)
            );
        }

        let (mut memory, paging) = setup(0x4007, 0x6000);
        let mut request = request(RamAccessOperation::Write {
            width: AccessWidth::Byte,
            value: 0xa5,
        });
        request.address = GuestLinearAddress::new((LINEAR & !0xfff) | 0xfff);
        let report = access_long64_4k_ram(&mut memory, &paging, request, |_| {});
        assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
        assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x4fff)), Ok(0xa5));
    }

    #[test]
    fn crossing_read_write_width_matrix_is_an_exact_no_effect_rejection() {
        for (width, bytes) in [
            (AccessWidth::Word, 2u8),
            (AccessWidth::Dword, 4),
            (AccessWidth::Qword, 8),
        ] {
            for offset in (0x1000 - u64::from(bytes) + 1)..0x1000 {
                for operation in [
                    RamAccessOperation::Read { width },
                    RamAccessOperation::Write {
                        width,
                        value: 0x8877_6655_4433_2211,
                    },
                ] {
                    let (mut memory, paging) = setup(0x4007 | (1 << 5) | (1 << 6), 0x6000);
                    let before = ram_snapshot(&memory, 0, 0x6000);
                    let mut access = request(operation);
                    access.address = GuestLinearAddress::new((LINEAR & !0xfff) | offset);
                    let mut callbacks = Vec::new();
                    let report = access_long64_4k_ram(&mut memory, &paging, access, |range| {
                        callbacks.push(range)
                    });

                    assert_eq!(
                        report.outcome,
                        Err(TrackedAccessError::CrossesPage {
                            address: access.address,
                            width: bytes,
                        })
                    );
                    assert!(report.trace.is_empty());
                    assert_eq!(report.ad_update_count, 0);
                    assert_eq!(report.ad_updates, [None; 5]);
                    assert!(callbacks.is_empty());
                    assert_eq!(ram_snapshot(&memory, 0, 0x6000), before);
                }
            }
        }
    }

    #[test]
    fn crossing_precedes_noncanonical_unmapped_tables_and_address_overflow_candidates() {
        let (mut memory, _) = setup(0x4007, 0x6000);
        let inaccessible =
            Long64ProbeConfig::new(0x8000_0033, 0x7000, CR4_PAE, EFER_LME | EFER_LMA | EFER_NXE)
                .unwrap();
        for (address, paging) in [
            (0x0000_8000_0000_0fffu64, None), // noncanonical, crosses
            (0xffff_ffff_ffff_ffffu64, None), // u64 endpoint overflow candidate
            (0x0000_0080_8060_4fffu64, Some(&inaccessible)), // unmapped root
        ] {
            let active_paging;
            let paging = match paging {
                Some(paging) => paging,
                None => {
                    active_paging = Long64ProbeConfig::new(
                        0x8000_0033,
                        0x1000,
                        CR4_PAE,
                        EFER_LME | EFER_LMA | EFER_NXE,
                    )
                    .unwrap();
                    &active_paging
                }
            };
            let mut access = request(RamAccessOperation::Write {
                width: AccessWidth::Qword,
                value: 0xfeed,
            });
            access.address = GuestLinearAddress::new(address);
            let before = ram_snapshot(&memory, 0, 0x6000);
            let mut callbacks = Vec::new();
            let report =
                access_long64_4k_ram(&mut memory, paging, access, |range| callbacks.push(range));
            assert_eq!(
                report.outcome,
                Err(TrackedAccessError::CrossesPage {
                    address: access.address,
                    width: 8,
                })
            );
            assert!(report.trace.is_empty());
            assert_eq!(report.ad_updates, [None; 5]);
            assert_eq!(report.ad_update_count, 0);
            assert!(callbacks.is_empty());
            assert_eq!(ram_snapshot(&memory, 0, 0x6000), before);
        }
    }

    #[test]
    fn crossing_refusal_is_independent_of_prospective_ram_hole_or_mmio() {
        for destination in [
            Some(MemoryRegionKind::Ram),
            None,
            Some(MemoryRegionKind::Mmio),
        ] {
            let mut regions = vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x6000).unwrap(),
                MemoryRegionKind::Ram,
            )];
            if let Some(kind) = destination {
                regions.push(MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(0x9000), 0x1000).unwrap(),
                    kind,
                ));
            }
            let map = MachineMemoryMap::new(36, regions).unwrap();
            let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x7000)).unwrap();
            for (address, value) in [
                (0x1008, 0x2007),
                (0x2010, 0x3007),
                (0x3018, 0x4007),
                (0x4020, 0x5007),
                (0x4028, 0x9007),
            ] {
                memory
                    .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                    .unwrap();
            }
            memory
                .write_u8(GuestPhysicalAddress::new(0x5fff), 0x6d, |_| {})
                .unwrap();
            if destination == Some(MemoryRegionKind::Ram) {
                memory
                    .write_bytes(GuestPhysicalAddress::new(0x9000), &[0xc7; 0x1000], |_| {})
                    .unwrap();
            }
            // The tracked RAM-only API has no BusDevice input; the MMIO case
            // can only remain untouched while the crossing request is refused.
            let paging = Long64ProbeConfig::new(
                0x8000_0033,
                0x1000,
                CR4_PAE,
                EFER_LME | EFER_LMA | EFER_NXE,
            )
            .unwrap();
            let before = ram_snapshot(&memory, 0, 0x6000);
            let extra_ram_before = (destination == Some(MemoryRegionKind::Ram))
                .then(|| ram_snapshot(&memory, 0x9000, 0x1000));
            let inaccessible_second_page = match destination {
                Some(MemoryRegionKind::Mmio) => Some(
                    memory
                        .read_u8(GuestPhysicalAddress::new(0x9000))
                        .unwrap_err(),
                ),
                None => Some(
                    memory
                        .read_u8(GuestPhysicalAddress::new(0x9000))
                        .unwrap_err(),
                ),
                _ => None,
            };
            let mut access = request(RamAccessOperation::Write {
                width: AccessWidth::Word,
                value: 0xbabe,
            });
            access.address = GuestLinearAddress::new((LINEAR & !0xfff) | 0xfff);
            let mut callbacks = Vec::new();
            let report =
                access_long64_4k_ram(&mut memory, &paging, access, |range| callbacks.push(range));
            assert_eq!(
                report.outcome,
                Err(TrackedAccessError::CrossesPage {
                    address: access.address,
                    width: 2,
                })
            );
            assert!(report.trace.is_empty());
            assert_eq!(report.ad_updates, [None; 5]);
            assert_eq!(report.ad_update_count, 0);
            assert!(callbacks.is_empty());
            assert_eq!(ram_snapshot(&memory, 0, 0x6000), before);
            assert_eq!(memory.read_u8(GuestPhysicalAddress::new(0x5fff)), Ok(0x6d));
            if let Some(expected) = extra_ram_before {
                assert_eq!(ram_snapshot(&memory, 0x9000, 0x1000), expected);
            }
            if let Some(expected_error) = inaccessible_second_page {
                assert_eq!(
                    memory.read_u8(GuestPhysicalAddress::new(0x9000)),
                    Err(expected_error)
                );
            }
        }
    }

    #[test]
    fn earlier_same_page_write_and_ad_effects_survive_later_crossing_call() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        let mut callbacks = Vec::new();
        let mut first = request(RamAccessOperation::Write {
            width: AccessWidth::Word,
            value: 0x1234,
        });
        first.address = GuestLinearAddress::new((LINEAR & !0xfff) | 0xffd);
        let first_report =
            access_long64_4k_ram(&mut memory, &paging, first, |range| callbacks.push(range));
        assert_eq!(first_report.outcome, Ok(TrackedAccessOutcome::Written));
        assert_eq!(first_report.ad_update_count, 5);
        assert_eq!(
            memory.read_u16(GuestPhysicalAddress::new(0x4ffd)),
            Ok(0x1234)
        );
        let after_first = ram_snapshot(&memory, 0, 0x6000);
        let committed_after_first = logged(&callbacks);

        let mut second = request(RamAccessOperation::Write {
            width: AccessWidth::Word,
            value: 0xabcd,
        });
        second.address = GuestLinearAddress::new((LINEAR & !0xfff) | 0xfff);
        let second_report =
            access_long64_4k_ram(&mut memory, &paging, second, |range| callbacks.push(range));
        assert_eq!(
            second_report.outcome,
            Err(TrackedAccessError::CrossesPage {
                address: second.address,
                width: 2,
            })
        );
        assert!(second_report.trace.is_empty());
        assert_eq!(second_report.ad_update_count, 0);
        assert_eq!(second_report.ad_updates, [None; 5]);
        assert_eq!(logged(&callbacks), committed_after_first);
        assert_eq!(ram_snapshot(&memory, 0, 0x6000), after_first);
    }

    #[test]
    fn adjacent_fetch_calls_keep_rip_and_report_second_page_fault_address() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        let page_base = LINEAR & !0xfff;
        let mut callbacks = Vec::new();
        let mut first = request(RamAccessOperation::Fetch);
        first.address = GuestLinearAddress::new(page_base | 0xfff);
        let first_report =
            access_long64_4k_ram(&mut memory, &paging, first, |range| callbacks.push(range));
        assert_eq!(first_report.outcome, Ok(TrackedAccessOutcome::Fetched(0)));
        assert_eq!(first_report.ad_update_count, 4);
        assert_eq!(first_report.trace.len(), 4);
        assert_eq!(
            first_report.trace.entries(),
            &[
                Some(WalkEntry {
                    level: PageTableLevel::Pml4,
                    index: 1,
                    entry_address: GuestPhysicalAddress::new(0x1008),
                    entry_value: 0x2007,
                }),
                Some(WalkEntry {
                    level: PageTableLevel::Pdpt,
                    index: 2,
                    entry_address: GuestPhysicalAddress::new(0x2010),
                    entry_value: 0x3007,
                }),
                Some(WalkEntry {
                    level: PageTableLevel::Pd,
                    index: 3,
                    entry_address: GuestPhysicalAddress::new(0x3018),
                    entry_value: 0x4007,
                }),
                Some(WalkEntry {
                    level: PageTableLevel::Pt,
                    index: 4,
                    entry_address: GuestPhysicalAddress::new(0x4020),
                    entry_value: 0x4007,
                }),
            ]
        );
        assert_eq!(
            first_report.ad_updates,
            [
                Some(AdUpdate {
                    level: PageTableLevel::Pml4,
                    address: GuestPhysicalAddress::new(0x1008),
                    before: 0x2007,
                    after: 0x2027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pdpt,
                    address: GuestPhysicalAddress::new(0x2010),
                    before: 0x3007,
                    after: 0x3027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pd,
                    address: GuestPhysicalAddress::new(0x3018),
                    before: 0x4007,
                    after: 0x4027,
                    bit: AdBit::Accessed,
                }),
                Some(AdUpdate {
                    level: PageTableLevel::Pt,
                    address: GuestPhysicalAddress::new(0x4020),
                    before: 0x4007,
                    after: 0x4027,
                    bit: AdBit::Accessed,
                }),
                None,
            ]
        );
        assert_eq!(
            logged(&callbacks),
            [(0x1008, 8), (0x2010, 8), (0x3018, 8), (0x4020, 8)]
        );
        let callbacks_after_first = logged(&callbacks);

        let mut second = request(RamAccessOperation::Fetch);
        second.address = GuestLinearAddress::new(page_base + 0x1000);
        let second_report =
            access_long64_4k_ram(&mut memory, &paging, second, |range| callbacks.push(range));
        let TrackedAccessError::PageFault { record, reason } = second_report
            .outcome
            .expect_err("missing next-page PTE must fault")
        else {
            panic!("expected tracked fetch page fault");
        };
        assert_eq!(
            reason,
            PageFaultReason::NotPresent {
                level: PageTableLevel::Pt
            }
        );
        assert_eq!(record.saved_rip(), RIP);
        assert_eq!(record.access(), AccessKind::Fetch);
        assert_eq!(record.cr2_value(), second.address.get());
        assert_eq!(record.fault().vector, 14);
        assert_eq!(record.fault().error_code, Some(1 << 4));
        assert_eq!(second_report.trace.len(), 4);
        assert_eq!(
            second_report.trace.entries(),
            &[
                Some(WalkEntry {
                    level: PageTableLevel::Pml4,
                    index: 1,
                    entry_address: GuestPhysicalAddress::new(0x1008),
                    entry_value: 0x2027,
                }),
                Some(WalkEntry {
                    level: PageTableLevel::Pdpt,
                    index: 2,
                    entry_address: GuestPhysicalAddress::new(0x2010),
                    entry_value: 0x3027,
                }),
                Some(WalkEntry {
                    level: PageTableLevel::Pd,
                    index: 3,
                    entry_address: GuestPhysicalAddress::new(0x3018),
                    entry_value: 0x4027,
                }),
                Some(WalkEntry {
                    level: PageTableLevel::Pt,
                    index: 5,
                    entry_address: GuestPhysicalAddress::new(0x4028),
                    entry_value: 0,
                }),
            ]
        );
        assert_eq!(second_report.ad_update_count, 0);
        assert_eq!(second_report.ad_updates, [None; 5]);
        assert_eq!(logged(&callbacks), callbacks_after_first);
        assert_eq!(entries(&memory), [0x2027, 0x3027, 0x4027, 0x4027]);
    }

    #[test]
    fn present_ps_at_pdpt_and_pd_is_rejected_without_descending_or_touching_operand() {
        for (level, address, value, expected_trace, expected_a_writes) in [
            (
                PageTableLevel::Pdpt,
                0x2010,
                0x4007 | ENTRY_PS_OR_PAT,
                vec![
                    (PageTableLevel::Pml4, 0x1008, 0x2007),
                    (PageTableLevel::Pdpt, 0x2010, 0x4007 | ENTRY_PS_OR_PAT),
                ],
                vec![(0x1008, 8)],
            ),
            (
                PageTableLevel::Pd,
                0x3018,
                0x4007 | ENTRY_PS_OR_PAT,
                vec![
                    (PageTableLevel::Pml4, 0x1008, 0x2007),
                    (PageTableLevel::Pdpt, 0x2010, 0x3007),
                    (PageTableLevel::Pd, 0x3018, 0x4007 | ENTRY_PS_OR_PAT),
                ],
                vec![(0x1008, 8), (0x2010, 8)],
            ),
        ] {
            let (mut memory, paging) = setup(0x4007, 0x6000);
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
            let before = entries(&memory);
            let expected_trace_len = expected_trace.len();

            let pure = probe_long64_4k(
                &memory,
                &paging,
                GuestLinearAddress::new(LINEAR),
                AccessKind::Read,
                request(RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                })
                .context,
            );
            // S4-10c: заменяет host-ошибку S4-10a для PDPTE.PS (теперь гостевой
            // #PF P|RSVD); PDE.PS остаётся UnsupportedPageSize.
            assert_eq!(
                pure.outcome,
                ps_probe_expected(level, address, value, AccessKind::Read, true, 0)
            );
            assert_eq!(pure.trace.len(), expected_trace.len());
            for (actual, (expected_level, expected_address, expected_value)) in pure
                .trace
                .entries()
                .iter()
                .take(expected_trace_len)
                .zip(expected_trace.iter().copied())
            {
                assert_eq!(
                    *actual,
                    Some(WalkEntry {
                        level: expected_level,
                        index: match expected_level {
                            PageTableLevel::Pml4 => 1,
                            PageTableLevel::Pdpt => 2,
                            PageTableLevel::Pd => 3,
                            PageTableLevel::Pt => unreachable!(),
                        },
                        entry_address: GuestPhysicalAddress::new(expected_address),
                        entry_value: expected_value,
                    })
                );
            }
            assert_eq!(
                entries(&memory),
                before,
                "pure probe mutated RAM at {level:?}"
            );

            let mut callbacks = Vec::new();
            let tracked = access_long64_4k_ram(
                &mut memory,
                &paging,
                request(RamAccessOperation::Write {
                    width: AccessWidth::Byte,
                    value: 0xaa,
                }),
                |range| callbacks.push(range),
            );
            assert_eq!(
                tracked.outcome,
                ps_tracked_expected(
                    level,
                    address,
                    value,
                    RamAccessOperation::Write {
                        width: AccessWidth::Byte,
                        value: 0xaa,
                    },
                    true,
                    0
                )
            );
            assert_eq!(tracked.trace.len(), expected_trace_len);
            for (actual, (expected_level, expected_address, expected_value)) in tracked
                .trace
                .entries()
                .iter()
                .take(expected_trace_len)
                .zip(expected_trace.iter().copied())
            {
                assert_eq!(
                    *actual,
                    Some(WalkEntry {
                        level: expected_level,
                        index: match expected_level {
                            PageTableLevel::Pml4 => 1,
                            PageTableLevel::Pdpt => 2,
                            PageTableLevel::Pd => 3,
                            PageTableLevel::Pt => unreachable!(),
                        },
                        entry_address: GuestPhysicalAddress::new(expected_address),
                        entry_value: expected_value,
                    })
                );
            }
            assert_eq!(tracked.ad_update_count, expected_a_writes.len());
            assert_eq!(
                logged(&callbacks),
                expected_a_writes
                    .iter()
                    .map(|(address, len)| (*address, *len))
                    .collect::<Vec<_>>()
            );
            let after = entries(&memory);
            assert_eq!(after[0], before[0] | (1 << 5));
            assert_eq!(
                after[1],
                if level == PageTableLevel::Pdpt {
                    value
                } else {
                    before[1] | (1 << 5)
                }
            );
            assert_eq!(
                after[2],
                if level == PageTableLevel::Pd {
                    value
                } else {
                    before[2]
                }
            );
            assert_eq!(
                after[3], before[3],
                "walk descended to the PT or touched operand"
            );
            assert_eq!(
                memory.read_u8(GuestPhysicalAddress::new(0x4abc)).unwrap(),
                0
            );
        }
    }

    #[test]
    fn present_ps_rejection_is_identical_for_read_write_and_fetch() {
        for (level, address, value, trace) in [
            (
                PageTableLevel::Pdpt,
                0x2010,
                0x4007 | ENTRY_PS_OR_PAT,
                vec![
                    (PageTableLevel::Pml4, 0x1008, 0x2007),
                    (PageTableLevel::Pdpt, 0x2010, 0x4007 | ENTRY_PS_OR_PAT),
                ],
            ),
            (
                PageTableLevel::Pd,
                0x3018,
                0x4007 | ENTRY_PS_OR_PAT,
                vec![
                    (PageTableLevel::Pml4, 0x1008, 0x2007),
                    (PageTableLevel::Pdpt, 0x2010, 0x3007),
                    (PageTableLevel::Pd, 0x3018, 0x4007 | ENTRY_PS_OR_PAT),
                ],
            ),
        ] {
            for (operation, access) in [
                (
                    RamAccessOperation::Read {
                        width: AccessWidth::Byte,
                    },
                    AccessKind::Read,
                ),
                (
                    RamAccessOperation::Write {
                        width: AccessWidth::Byte,
                        value: 0x5a,
                    },
                    AccessKind::Write,
                ),
                (RamAccessOperation::Fetch, AccessKind::Fetch),
            ] {
                let (mut memory, paging) = setup(0x4007, 0x6000);
                memory
                    .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                    .unwrap();
                let before = entries(&memory);
                let pure = probe_long64_4k(
                    &memory,
                    &paging,
                    GuestLinearAddress::new(LINEAR),
                    access,
                    request(operation).context,
                );
                // S4-10c: заменяет host-ошибку S4-10a для PDPTE.PS.
                assert_eq!(
                    pure.outcome,
                    ps_probe_expected(level, address, value, access, true, 0)
                );
                assert_eq!(pure.trace.len(), trace.len());
                for (actual, (expected_level, expected_address, expected_value)) in pure
                    .trace
                    .entries()
                    .iter()
                    .take(trace.len())
                    .zip(trace.iter().copied())
                {
                    assert_eq!(
                        actual.map(|entry| (
                            entry.level,
                            entry.entry_address.get(),
                            entry.entry_value
                        )),
                        Some((expected_level, expected_address, expected_value))
                    );
                }
                assert_eq!(entries(&memory), before);

                let mut callbacks = Vec::new();
                let tracked =
                    access_long64_4k_ram(&mut memory, &paging, request(operation), |range| {
                        callbacks.push(range)
                    });
                assert_eq!(
                    tracked.outcome,
                    ps_tracked_expected(level, address, value, operation, true, 0)
                );
                assert_eq!(tracked.trace.len(), trace.len());
                for (actual, (expected_level, expected_address, expected_value)) in tracked
                    .trace
                    .entries()
                    .iter()
                    .take(trace.len())
                    .zip(trace.iter().copied())
                {
                    assert_eq!(
                        actual.map(|entry| (
                            entry.level,
                            entry.entry_address.get(),
                            entry.entry_value
                        )),
                        Some((expected_level, expected_address, expected_value))
                    );
                }
                assert_eq!(
                    entries(&memory),
                    [
                        before[0] | 0x20,
                        before[1] | if level == PageTableLevel::Pd { 0x20 } else { 0 },
                        before[2],
                        before[3]
                    ]
                );
                assert_eq!(
                    logged(&callbacks),
                    if level == PageTableLevel::Pdpt {
                        vec![(0x1008, 8)]
                    } else {
                        vec![(0x1008, 8), (0x2010, 8)]
                    }
                );
                assert_eq!(tracked.ad_update_count, callbacks.len());
                assert_eq!(
                    tracked.ad_updates,
                    if level == PageTableLevel::Pdpt {
                        [
                            Some(AdUpdate {
                                level: PageTableLevel::Pml4,
                                address: GuestPhysicalAddress::new(0x1008),
                                before: 0x2007,
                                after: 0x2027,
                                bit: AdBit::Accessed,
                            }),
                            None,
                            None,
                            None,
                            None,
                        ]
                    } else {
                        [
                            Some(AdUpdate {
                                level: PageTableLevel::Pml4,
                                address: GuestPhysicalAddress::new(0x1008),
                                before: 0x2007,
                                after: 0x2027,
                                bit: AdBit::Accessed,
                            }),
                            Some(AdUpdate {
                                level: PageTableLevel::Pdpt,
                                address: GuestPhysicalAddress::new(0x2010),
                                before: 0x3007,
                                after: 0x3027,
                                bit: AdBit::Accessed,
                            }),
                            None,
                            None,
                            None,
                        ]
                    }
                );
                assert_eq!(
                    memory.read_u8(GuestPhysicalAddress::new(0x4abc)).unwrap(),
                    0
                );
            }
        }
    }

    #[test]
    fn present_ps_after_preset_ancestor_accessed_bit_has_no_duplicate_callback() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x1008), 0x2027, |_| {})
            .unwrap();
        memory
            .write_u64(
                GuestPhysicalAddress::new(0x2010),
                0x4007 | ENTRY_PS_OR_PAT,
                |_| {},
            )
            .unwrap();
        let rejected_before = memory.read_u64(GuestPhysicalAddress::new(0x2010)).unwrap();
        let mut callbacks = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |range| callbacks.push(range),
        );
        // S4-10c: заменяет host-ошибку S4-10a: PDPTE.PS=1 -> гостевой #PF P|RSVD.
        assert_eq!(
            report.outcome,
            ps_tracked_expected(
                PageTableLevel::Pdpt,
                0x2010,
                rejected_before,
                RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                },
                true,
                0
            )
        );
        assert_eq!(report.trace.len(), 2);
        assert_eq!(report.ad_update_count, 0);
        assert!(callbacks.is_empty());
        assert_eq!(
            memory.read_u64(GuestPhysicalAddress::new(0x2010)).unwrap(),
            rejected_before
        );
        assert_eq!(
            memory.read_u8(GuestPhysicalAddress::new(0x4abc)).unwrap(),
            0
        );
    }

    #[test]
    fn present_ps_wins_over_reserved_address_bits_and_nx_when_nxe_is_disabled() {
        let paging =
            Long64ProbeConfig::new(0x8000_0033, 0x1000, CR4_PAE, EFER_LME | EFER_LMA).unwrap();
        for (level, address) in [(PageTableLevel::Pdpt, 0x2010), (PageTableLevel::Pd, 0x3018)] {
            for extra in [1u64 << 36, ENTRY_NX] {
                let value = 0x4007 | ENTRY_PS_OR_PAT | extra;
                let (mut memory, _) = setup(0x4007, 0x6000);
                memory
                    .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                    .unwrap();
                let pure = probe_long64_4k(
                    &memory,
                    &paging,
                    GuestLinearAddress::new(LINEAR),
                    AccessKind::Write,
                    request(RamAccessOperation::Read {
                        width: AccessWidth::Byte,
                    })
                    .context,
                );
                // S4-10c: заменяет host-ошибку S4-10a для PDPTE.PS. Теперь PDPTE.PS
                // не «побеждает» reserved-биты, а входит в offending_bits вместе с
                // ними; PDE.PS по-прежнему UnsupportedPageSize.
                assert_eq!(
                    pure.outcome,
                    ps_probe_expected(level, address, value, AccessKind::Write, false, extra)
                );
                assert_eq!(
                    pure.trace
                        .entries()
                        .get(pure.trace.len() - 1)
                        .and_then(|entry| *entry),
                    Some(WalkEntry {
                        level,
                        index: if level == PageTableLevel::Pdpt { 2 } else { 3 },
                        entry_address: GuestPhysicalAddress::new(address),
                        entry_value: value,
                    })
                );

                let mut callbacks = Vec::new();
                let tracked = access_long64_4k_ram(
                    &mut memory,
                    &paging,
                    request(RamAccessOperation::Write {
                        width: AccessWidth::Byte,
                        value: 0x77,
                    }),
                    |range| callbacks.push(range),
                );
                assert_eq!(
                    tracked.outcome,
                    ps_tracked_expected(
                        level,
                        address,
                        value,
                        RamAccessOperation::Write {
                            width: AccessWidth::Byte,
                            value: 0x77,
                        },
                        false,
                        extra
                    )
                );
                assert_eq!(
                    memory.read_u64(GuestPhysicalAddress::new(address)).unwrap(),
                    value
                );
                assert_eq!(
                    logged(&callbacks),
                    if level == PageTableLevel::Pdpt {
                        vec![(0x1008, 8)]
                    } else {
                        vec![(0x1008, 8), (0x2010, 8)]
                    }
                );
            }
        }
    }

    #[test]
    fn unsupported_ps_does_not_read_an_unmapped_descendant_or_operand() {
        for (level, ram_end, address, prefix) in [
            (PageTableLevel::Pdpt, 0x3000, 0x2010, vec![(0x1008, 0x2007)]),
            (
                PageTableLevel::Pd,
                0x4000,
                0x3018,
                vec![(0x1008, 0x2007), (0x2010, 0x3007)],
            ),
        ] {
            let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), ram_end).unwrap();
            let map =
                MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
                    .unwrap();
            let mut memory = PhysicalMemory::new(&map, RamLimits::new(ram_end)).unwrap();
            for (table_address, entry) in prefix {
                memory
                    .write_u64(GuestPhysicalAddress::new(table_address), entry, |_| {})
                    .unwrap();
            }
            let value = 0x4007 | ENTRY_PS_OR_PAT;
            memory
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
            let paging = Long64ProbeConfig::new(
                0x8000_0033,
                0x1000,
                CR4_PAE,
                EFER_LME | EFER_LMA | EFER_NXE,
            )
            .unwrap();
            assert_eq!(
                memory.read_u64(GuestPhysicalAddress::new(0x4000)),
                Err(MemoryAccessError::Unmapped(GuestPhysicalAddress::new(
                    0x4000
                )))
            );
            assert_eq!(
                memory.read_u8(GuestPhysicalAddress::new(0x4abc)),
                Err(MemoryAccessError::Unmapped(GuestPhysicalAddress::new(
                    0x4abc
                )))
            );

            let pure = probe_long64_4k(
                &memory,
                &paging,
                GuestLinearAddress::new(LINEAR),
                AccessKind::Read,
                request(RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                })
                .context,
            );
            // S4-10c: заменяет host-ошибку S4-10a для PDPTE.PS.
            assert_eq!(
                pure.outcome,
                ps_probe_expected(level, address, value, AccessKind::Read, true, 0)
            );

            let mut callbacks = Vec::new();
            let tracked = access_long64_4k_ram(
                &mut memory,
                &paging,
                request(RamAccessOperation::Read {
                    width: AccessWidth::Byte,
                }),
                |range| callbacks.push(range),
            );
            assert_eq!(
                tracked.outcome,
                ps_tracked_expected(
                    level,
                    address,
                    value,
                    RamAccessOperation::Read {
                        width: AccessWidth::Byte,
                    },
                    true,
                    0
                )
            );
            assert_eq!(
                tracked.trace.len(),
                if level == PageTableLevel::Pdpt { 2 } else { 3 }
            );
            assert_eq!(
                logged(&callbacks),
                if level == PageTableLevel::Pdpt {
                    vec![(0x1008, 8)]
                } else {
                    vec![(0x1008, 8), (0x2010, 8)]
                }
            );
        }
    }

    #[test]
    fn rejected_write_preserves_preset_accessed_and_dirty_bits() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        let rejected_value = 0x4007 | ENTRY_PS_OR_PAT | (1 << 5) | (1 << 6);
        memory
            .write_u64(GuestPhysicalAddress::new(0x1008), 0x2027, |_| {})
            .unwrap();
        memory
            .write_u64(GuestPhysicalAddress::new(0x2010), rejected_value, |_| {})
            .unwrap();
        let mut callbacks = Vec::new();
        let report = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: 0x11,
            }),
            |range| callbacks.push(range),
        );
        // S4-10c: заменяет host-ошибку S4-10a: гостевой #PF P|RSVD|W (0x0b).
        assert_eq!(
            report.outcome,
            ps_tracked_expected(
                PageTableLevel::Pdpt,
                0x2010,
                rejected_value,
                RamAccessOperation::Write {
                    width: AccessWidth::Byte,
                    value: 0x11,
                },
                true,
                0
            )
        );
        assert_eq!(report.ad_update_count, 0);
        assert_eq!(report.ad_updates, [None; 5]);
        assert!(callbacks.is_empty());
        assert_eq!(
            memory.read_u64(GuestPhysicalAddress::new(0x1008)).unwrap(),
            0x2027
        );
        assert_eq!(
            memory.read_u64(GuestPhysicalAddress::new(0x2010)).unwrap(),
            rejected_value
        );
        assert_eq!(
            memory.read_u8(GuestPhysicalAddress::new(0x4abc)).unwrap(),
            0
        );
    }

    #[test]
    fn nonpresent_ps_is_a_page_fault_and_pml4_pt_bit_seven_keep_their_roles() {
        let (mut memory, paging) = setup(0x4007, 0x6000);
        memory
            .write_u64(GuestPhysicalAddress::new(0x2010), ENTRY_PS_OR_PAT, |_| {})
            .unwrap();
        let pure = probe_long64_4k(
            &memory,
            &paging,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            })
            .context,
        );
        assert!(matches!(
            pure.outcome,
            Ok(ProbeOutcome::PageFault(PageFaultProbe {
                error_code: 0,
                reason: PageFaultReason::NotPresent {
                    level: PageTableLevel::Pdpt
                },
                ..
            }))
        ));
        assert_eq!(pure.trace.len(), 2);
        let mut callbacks = Vec::new();
        let tracked = access_long64_4k_ram(
            &mut memory,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |range| callbacks.push(range),
        );
        assert!(matches!(
            tracked.outcome,
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::NotPresent {
                    level: PageTableLevel::Pdpt
                },
                ..
            })
        ));
        assert_eq!(tracked.trace.len(), 2);
        assert_eq!(tracked.ad_update_count, 1);
        assert_eq!(
            tracked.ad_updates,
            [
                Some(AdUpdate {
                    level: PageTableLevel::Pml4,
                    address: GuestPhysicalAddress::new(0x1008),
                    before: 0x2007,
                    after: 0x2027,
                    bit: AdBit::Accessed,
                }),
                None,
                None,
                None,
                None,
            ]
        );
        assert_eq!(logged(&callbacks), [(0x1008, 8)]);
        assert_eq!(
            memory.read_u64(GuestPhysicalAddress::new(0x1008)).unwrap(),
            0x2027
        );
        assert_eq!(
            memory.read_u64(GuestPhysicalAddress::new(0x2010)).unwrap(),
            ENTRY_PS_OR_PAT
        );

        let mut pml4_ps = setup(0x4007, 0x6000).0;
        pml4_ps
            .write_u64(
                GuestPhysicalAddress::new(0x1008),
                0x2007 | ENTRY_PS_OR_PAT,
                |_| {},
            )
            .unwrap();
        let pml4 = probe_long64_4k(
            &pml4_ps,
            &paging,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            })
            .context,
        );
        assert!(matches!(
            pml4.outcome,
            Ok(ProbeOutcome::PageFault(PageFaultProbe {
                reason: PageFaultReason::ReservedBits {
                    level: PageTableLevel::Pml4,
                    offending_bits: ENTRY_PS_OR_PAT
                },
                ..
            }))
        ));
        assert_eq!(pml4.trace.len(), 1);
        let mut pml4_tracked = pml4_ps;
        let pml4_access = access_long64_4k_ram(
            &mut pml4_tracked,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |_| {},
        );
        assert!(matches!(
            pml4_access.outcome,
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::ReservedBits {
                    level: PageTableLevel::Pml4,
                    offending_bits: ENTRY_PS_OR_PAT
                },
                ..
            })
        ));
        assert_eq!(pml4_access.trace.len(), 1);
        assert_eq!(pml4_access.ad_update_count, 0);

        let (mut pt_pat, paging) = setup(0x4007 | ENTRY_PS_OR_PAT, 0x6000);
        pt_pat
            .write_u8(GuestPhysicalAddress::new(0x4abc), 0x5a, |_| {})
            .unwrap();
        let pt = probe_long64_4k(
            &pt_pat,
            &paging,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            })
            .context,
        );
        assert!(matches!(
            pt.outcome,
            Ok(ProbeOutcome::Translated(translation)) if translation.physical.get() == 0x4abc
        ));
        assert_eq!(pt.trace.len(), 4);
        let pt_access = access_long64_4k_ram(
            &mut pt_pat,
            &paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            |_| {},
        );
        assert_eq!(pt_access.outcome, Ok(TrackedAccessOutcome::Read(0x5a)));
        assert_eq!(pt_access.trace.len(), 4);
        assert_eq!(pt_access.ad_update_count, 4);
    }

    #[test]
    fn s4_10c_pdpte_ps_is_guest_rsvd_fault_with_a_only_on_pml4e() {
        // Literal PFEC values (Intel SDM vol. 3, "Page-Fault Error Code"):
        // P=bit0, W/R=bit1, U/S=bit2, RSVD=bit3, I/D=bit4 (only with NXE=1).
        let cases = [
            (PrivilegeLevel::Ring0, AccessKind::Read, true, 0x09u32),
            (PrivilegeLevel::Ring0, AccessKind::Write, true, 0x0b),
            (PrivilegeLevel::Ring0, AccessKind::Fetch, true, 0x19),
            (PrivilegeLevel::Ring0, AccessKind::Fetch, false, 0x09),
            (PrivilegeLevel::Ring3, AccessKind::Read, true, 0x0d),
            (PrivilegeLevel::Ring3, AccessKind::Write, true, 0x0f),
            (PrivilegeLevel::Ring3, AccessKind::Fetch, true, 0x1d),
            (PrivilegeLevel::Ring3, AccessKind::Fetch, false, 0x0d),
        ];
        for allow_2m in [false, true] {
            for (privilege_level, access, nxe, expected_pfec) in cases {
                let (mut memory, _) =
                    setup_entries([0x2007, 0x3007 | ENTRY_PS_OR_PAT, 0x4007, 0x5007], 0x6000);
                let paging = paging_config(false, nxe);
                let context = privilege(privilege_level);
                let before = entries(&memory);
                let mut callbacks = Vec::new();
                let report = if allow_2m {
                    access_long64_2m_ram(
                        &mut memory,
                        &paging,
                        request_as(context, operation(access)),
                        |range| callbacks.push(range),
                    )
                } else {
                    access_long64_4k_ram(
                        &mut memory,
                        &paging,
                        request_as(context, operation(access)),
                        |range| callbacks.push(range),
                    )
                };
                let label = format!("2m={allow_2m} {privilege_level:?} {access:?} nxe={nxe}");
                // S4-10c: replaces the S4-10a host error; this must never come back.
                assert!(
                    !matches!(
                        report.outcome,
                        Err(TrackedAccessError::UnsupportedPageSize { .. })
                    ),
                    "{label}: PDPTE.PS returned the host error"
                );
                match &report.outcome {
                    Err(TrackedAccessError::PageFault { record, reason }) => {
                        assert_eq!(record.fault().vector, 14, "{label}");
                        assert_eq!(record.fault().error_code, Some(expected_pfec), "{label}");
                        assert_eq!(record.cr2_value(), LINEAR, "{label}");
                        assert_eq!(record.saved_rip(), RIP, "{label}");
                        assert_eq!(
                            *reason,
                            PageFaultReason::ReservedBits {
                                level: PageTableLevel::Pdpt,
                                offending_bits: ENTRY_PS_OR_PAT,
                            },
                            "{label}"
                        );
                    }
                    other => panic!("{label}: expected guest #PF, got {other:?}"),
                }
                // Walk stops at the PDPTE; A only on the successful PML4E ancestor.
                assert_eq!(report.trace.len(), 2, "{label}");
                assert_eq!(report.ad_update_count, 1, "{label}");
                assert_eq!(
                    report.ad_updates[0],
                    Some(AdUpdate {
                        level: PageTableLevel::Pml4,
                        address: GuestPhysicalAddress::new(0x1008),
                        before: 0x2007,
                        after: 0x2027,
                        bit: AdBit::Accessed,
                    }),
                    "{label}"
                );
                assert_eq!(&report.ad_updates[1..], &[None; 4], "{label}");
                assert_eq!(logged(&callbacks), [(0x1008, 8)], "{label}");
                let after = entries(&memory);
                assert_eq!(after[0], before[0] | 0x20, "{label}");
                assert_eq!(after[1], before[1], "PDPTE got A/D: {label}");
                assert_eq!(after[2..], before[2..], "{label}");
                // No operand effect: the would-be target page stays zero.
                assert_eq!(ram_snapshot(&memory, 0x5000, 0x1000), vec![0; 0x1000]);
            }
        }
    }
}

/// Adds instruction restart metadata to a pure page-walk result. This does not
/// deliver the exception, update CR2, or mutate CPU or guest memory state.
pub fn probe_instruction_access(
    memory: &PhysicalMemory,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    address: GuestLinearAddress,
    access: AccessKind,
    context: AccessContext,
) -> InstructionProbeReport {
    let report = probe_long64_4k(memory, paging, address, access, context);
    let outcome = report.outcome.map(|outcome| match outcome {
        ProbeOutcome::Translated(translation) => InstructionProbeOutcome::Translated(translation),
        ProbeOutcome::PageFault(probe) => InstructionProbeOutcome::PageFault {
            record: PageFaultRecord::new(
                instruction_rip,
                probe.access,
                context,
                probe.error_code,
                probe.linear.get(),
            ),
            reason: probe.reason,
        },
    });
    InstructionProbeReport {
        outcome,
        trace: report.trace,
    }
}

/// Page-table structures may live only in RAM. A `Framebuffer` region has
/// readable backing but is rejected here, so the walkers neither read an entry
/// from it nor write A/D bits into it.
fn read_table_entry(
    memory: &PhysicalMemory,
    entry_address: GuestPhysicalAddress,
) -> Result<u64, MemoryAccessError> {
    if memory.locate_region_kind(entry_address, 8)? == MemoryRegionKind::Framebuffer {
        return Err(MemoryAccessError::UnreadableRegion(
            MemoryRegionKind::Framebuffer,
        ));
    }
    memory.read_u64(entry_address)
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
    fn cpu_state_snapshot_requires_cr0_then_cr4_and_preserves_raw_values() {
        let mut state = od_isa::state::LongModeState::default();
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::MissingCpuRegister { register: "CR0" })
        );

        state.cr0 = Some(0x8001_0033);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::MissingCpuRegister { register: "CR4" })
        );

        state.cr4 = Some(0x620);
        state.cr3 = Some(ROOT | 0x18 | 0x7e0);
        let snapshot = Long64ProbeConfig::from_cpu_state(
            &state,
            ROOT | 0x18 | 0x7e0,
            EFER_LME | EFER_LMA | EFER_NXE,
        )
        .unwrap();
        assert_eq!(snapshot.cr0(), 0x8001_0033);
        assert_eq!(snapshot.cr3(), ROOT | 0x18 | 0x7e0);
        assert_eq!(snapshot.cr4(), 0x620);
        assert_eq!(snapshot.efer(), 0xd00);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::Cr3Mismatch {
                cpu_cr3: ROOT | 0x18 | 0x7e0,
                snapshot_cr3: ROOT,
            })
        );
    }

    #[test]
    fn cpu_state_snapshot_admits_cr4_pse_as_a_snapshot_only_bit() {
        // S4-38L7c: PSE (bit 4) is admitted with PAE; it does not change IA-32e translation.
        let mut state = od_isa::state::LongModeState::default();
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(CR4_PAE | (1 << 4));
        let snapshot =
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA).unwrap();
        assert_eq!(snapshot.cr4(), 0x30);
        // PSE without PAE is still rejected.
        state.cr4 = Some(1 << 4);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::RequiredBits {
                register: "CR4",
                bits: CR4_PAE,
            })
        );
    }

    #[test]
    fn cpu_state_snapshot_admits_cr4_pge_as_a_snapshot_only_bit() {
        // S4-38L7d: PGE (bit 7) is admitted with PAE; the model has no TLB and the walker
        // never reads CR4 or the G bit of an entry.
        let mut state = od_isa::state::LongModeState::default();
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(CR4_PAE | CR4_PSE | CR4_PGE);
        let snapshot =
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA).unwrap();
        assert_eq!(snapshot.cr4(), 0xb0);
        state.cr4 = Some(CR4_PAE | CR4_PGE);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA)
                .unwrap()
                .cr4(),
            0xa0
        );
        // PGE without PAE is still rejected.
        state.cr4 = Some(CR4_PGE);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::RequiredBits {
                register: "CR4",
                bits: CR4_PAE,
            })
        );
    }

    #[test]
    fn cpu_state_snapshot_delegates_invalid_raw_inputs_to_existing_validation() {
        let mut state = od_isa::state::LongModeState::default();
        state.cr0 = Some(0x8000_0023);
        state.cr4 = Some(0);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT | (1 << 36), EFER_LME),
            Err(ProbeConfigError::Cr0EtRequired { value: 0x8000_0023 })
        );

        state.cr0 = Some(0x8000_0033);
        state.cr4 = Some(CR4_PAE | (1 << 12));
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME | EFER_LMA),
            Err(ProbeConfigError::UnsupportedBits {
                register: "CR4",
                value: 0x1020,
                offending_bits: 0x1000,
            })
        );

        state.cr4 = Some(CR4_PAE);
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER_LME),
            Err(ProbeConfigError::RequiredBits {
                register: "EFER",
                bits: EFER_LME | EFER_LMA,
            })
        );
        assert_eq!(
            Long64ProbeConfig::from_cpu_state(&state, ROOT | (1 << 36), EFER_LME | EFER_LMA,),
            Err(ProbeConfigError::InvalidCr3 {
                value: ROOT | (1 << 36),
                offending_bits: 1 << 36,
            })
        );
    }

    #[test]
    fn cpu_state_snapshots_are_immutable_and_roots_select_distinct_translations() {
        let mut state = od_isa::state::LongModeState::default();
        state.gpr[3] = 0xfeed_face_cafe_beef;
        state.rip = 0x1234_5678;
        state.rflags = 0x246;
        state.fs_base = 0x0000_7fff_1234_0000;
        state.cr0 = Some(0x8000_0033);
        state.cr4 = Some(CR4_PAE);
        let original_state = state.clone();

        let first = Long64ProbeConfig::from_cpu_state(&state, 0x1000, EFER_LME | EFER_LMA).unwrap();
        assert_eq!(state, original_state);
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(CR4_PAE | CR4_OSFXSR);
        let second =
            Long64ProbeConfig::from_cpu_state(&state, 0x5000, EFER_LME | EFER_LMA).unwrap();
        assert_eq!(first.cr0(), 0x8000_0033);
        assert_eq!(first.cr4(), CR4_PAE);
        assert_eq!(first.cr3(), 0x1000);
        assert_eq!(second.cr0(), 0x8001_0033);
        assert_eq!(second.cr4(), CR4_PAE | CR4_OSFXSR);
        assert_eq!(second.cr3(), 0x5000);

        let mut mem = tables(memory(0xa000), 0x5007);
        for (address, value) in [
            (0x5008, 0x6007),
            (0x6010, 0x7007),
            (0x7018, 0x8007),
            (0x8020, 0x9007),
        ] {
            mem.write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        let first_translation = result(probe_long64_4k(
            &mem,
            &first,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        let second_translation = result(probe_long64_4k(
            &mem,
            &second,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        ));
        assert!(matches!(first_translation, ProbeOutcome::Translated(t)
            if t.physical.get() == 0x5abc && t.leaf_entry_address.get() == 0x4020));
        assert!(matches!(second_translation, ProbeOutcome::Translated(t)
            if t.physical.get() == 0x9abc && t.leaf_entry_address.get() == 0x8020));

        assert_eq!(original_state.gpr[3], 0xfeed_face_cafe_beef);
        assert_eq!(original_state.rip, 0x1234_5678);
        assert_eq!(original_state.rflags, 0x246);
        assert_eq!(original_state.fs_base, 0x0000_7fff_1234_0000);
        assert_eq!(original_state.cr0, Some(0x8000_0033));
        assert_eq!(original_state.cr4, Some(CR4_PAE));
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

    #[test]
    fn instruction_probe_constructs_only_page_fault_records_and_preserves_context() {
        use od_isa::memory::{AccessOrigin, InstructionMode};

        let paging = cfg(false, true);
        let rip = 0x1234;
        let fault_linear = GuestLinearAddress::new(LINEAR);
        let descriptor_context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring3,
            origin: AccessOrigin::DescriptorTable,
        };
        let mem = tables(memory(0x5000), 0x4003 | (1 << 5) | (1 << 6) | ENTRY_NX_BIT);
        let before = [0x1008, 0x2010, 0x3018, 0x4020]
            .map(|address| mem.read_u64(GuestPhysicalAddress::new(address)).unwrap());
        let report = probe_instruction_access(
            &mem,
            &paging,
            rip,
            fault_linear,
            AccessKind::Fetch,
            descriptor_context,
        );
        match report.outcome.unwrap() {
            InstructionProbeOutcome::PageFault { record, reason } => {
                assert_eq!(record.fault().vector, 14);
                assert_eq!(record.fault().error_code, Some(0x11));
                assert_eq!(record.saved_rip(), rip);
                assert_ne!(record.saved_rip(), record.cr2_value());
                assert_eq!(record.cr2_value(), LINEAR);
                assert_eq!(record.access(), AccessKind::Fetch);
                assert_eq!(record.context(), descriptor_context);
                assert_eq!(record.context().page_privilege(), PrivilegeLevel::Ring0);
                assert_eq!(
                    reason,
                    PageFaultReason::Protection {
                        user_denied: false,
                        write_denied: false,
                        execute_denied: true,
                    }
                );
            }
            other => panic!("expected page-fault record, got {other:?}"),
        }
        let after = [0x1008, 0x2010, 0x3018, 0x4020]
            .map(|address| mem.read_u64(GuestPhysicalAddress::new(address)).unwrap());
        assert_eq!(after, before);

        let translated = probe_instruction_access(
            &mem,
            &paging,
            rip,
            fault_linear,
            AccessKind::Read,
            context(PrivilegeLevel::Ring0),
        );
        assert!(matches!(
            translated.outcome,
            Ok(InstructionProbeOutcome::Translated(_))
        ));
    }

    #[test]
    fn instruction_probe_keeps_non_page_fault_results_distinct() {
        let paging = cfg(false, false);
        let context = context(PrivilegeLevel::Ring0);
        let mem = memory(0x5000);
        let noncanonical = probe_instruction_access(
            &mem,
            &paging,
            0x1000,
            GuestLinearAddress::new(0x0000_8000_0000_0000),
            AccessKind::Read,
            context,
        );
        assert!(matches!(
            noncanonical.outcome,
            Err(ProbeError::NonCanonical(_))
        ));

        let mut large_page = tables(memory(0x5000), 0x4007);
        large_page
            .write_u64(
                GuestPhysicalAddress::new(0x3018),
                0x4007 | ENTRY_PS_OR_PAT,
                |_| {},
            )
            .unwrap();
        let unsupported = probe_instruction_access(
            &large_page,
            &paging,
            0x1000,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context,
        );
        assert!(matches!(
            unsupported.outcome,
            Err(ProbeError::UnsupportedPageSize { .. })
        ));

        let mut short_memory = memory(0x2000);
        short_memory
            .write_u64(GuestPhysicalAddress::new(0x1008), 0x2007, |_| {})
            .unwrap();
        let table_read = probe_instruction_access(
            &short_memory,
            &paging,
            0x1000,
            GuestLinearAddress::new(LINEAR),
            AccessKind::Read,
            context,
        );
        assert!(matches!(
            table_read.outcome,
            Err(ProbeError::Access(ProbeAccessError::TableRead { .. }))
        ));
    }
}
