//! S7-32, ADR-0023 slice C1 (shared by the C1 and C2a tests): the encoding sweep, the sweep memory,
//! the machine and the set the sweep runs on, and the outcome digest.
//!
//! - [`encodings`]: the reduced outcome sweep (plan C1-0 item 1): the maps `Primary`, `0F`, `0F 38`
//!   and `0F 3A` × fifteen prefix sets × ModRM (register forms with every `reg` and `rm` in
//!   {0, 1, 7}; memory forms with every `reg`: `[rdi]`, `[rdi+0x10]`, `[rip+0x40]`,
//!   `[rdi+rsi*2+0x10]`) and an eight-byte tail of `0x11`; each candidate is cut to the length the
//!   decoder reports, the candidates the decoder rejects are skipped.
//! - [`SweepMemory`]: linear = physical 4 KiB pages with user/writable/present flags; a missing or
//!   protected page is the architectural `#PF` of Vol. 3A "Event 14"; with on-demand mapping (on in
//!   the C1-0 baseline) a missing page touched by a data or stack access becomes a user writable page
//!   with the pattern `0x5a ^ (address & 0xff)`; a log of writes and of every data access.
//! - [`machine`] and [`set`]: a Windows-shaped GDT, IDT and TSS, user code, data and stack pages,
//!   distinct GPRs and the set with every member the sweep needs.
//! - [`outcome_fields`] / [`outcome_digest`]: the expanded outcome and its FNV-1a 64 digest.
//!
//! FROZEN after the C1-0 baseline (`tests/data/c1_cpl0_baseline.txt`, written on G0 = `ad625e3`):
//! `machine()`, `set()`, `Cells::new()`, `encodings()`, the [`SweepMemory`] rules and the field list of
//! [`outcome_fields`] define the recorded outcomes. Extend them only with new functions or with new
//! parameters whose default is the G0 value; any change of these breaks the CPL0 baseline.

#![allow(dead_code)]

use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetInputError, Cpl0ProfileSetResult,
    GuestFpState, MovCr3Long64V1, RunConfig, run_with_cpl0_profile_set_and_pio,
    run_with_cpl3_profile_set,
};
use od_isa::msr::{IA32_MISC_ENABLE_RESET, IA32_PAT_RESET, McaState, MsrProfile, MtrrState};
use od_isa::state::{Gdtr, Idtr, SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CliProfile,
    CodeSegmentDescriptorCache, CpuActivity, DataSegmentDescriptorCache, FsDescriptorCache,
    GuestMemory, HaltProfile, InstructionMode, LongModeState, MemoryError, OperandSize, PortIo,
    PrivilegeLevel, Register, StiProfile, StiShadow, TaskRegisterCache, page_fault_error_code,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

// ---- the encoding sweep ---------------------------------------------------------------------------

/// The opcode maps of the sweep, in sweep order.
pub const MAPS: [(&str, &[u8]); 4] = [
    ("P", &[]),
    ("0F", &[0x0f]),
    ("0F38", &[0x0f, 0x38]),
    ("0F3A", &[0x0f, 0x3a]),
];

/// The prefix sets of the reduced sweep (plan C1-0 item 1), in sweep order.
pub const PREFIX_SETS: [&[u8]; 15] = [
    &[],
    &[0x66],
    &[0xf2],
    &[0xf3],
    &[0x48],
    &[0x47],
    &[0x4f],
    &[0x67],
    &[0x64],
    &[0x65],
    &[0xf0],
    &[0x66, 0x48],
    &[0xf3, 0x48],
    &[0xf2, 0x48],
    &[0xf0, 0x48],
];

/// The ModRM part of the reduced sweep: the ModRM byte and what follows it before the tail.
pub fn modrm_parts() -> Vec<Vec<u8>> {
    let mut parts = Vec::new();
    for reg in 0..8u8 {
        for rm in [0u8, 1, 7] {
            parts.push(vec![0xc0 | (reg << 3) | rm]);
        }
        // [rdi] (mod 00, rm 7)
        parts.push(vec![(reg << 3) | 7]);
        // [rdi+0x10] (mod 01, rm 7, disp8)
        parts.push(vec![0x40 | (reg << 3) | 7, 0x10]);
        // [rip+0x40] (mod 00, rm 5, disp32)
        parts.push(vec![(reg << 3) | 5, 0x40, 0, 0, 0]);
        // [rdi+rsi*2+0x10] (mod 01, rm 4, SIB scale 2 index RSI base RDI, disp8)
        parts.push(vec![0x44 | (reg << 3), 0x77, 0x10]);
    }
    parts
}

/// The tail after the ModRM part (immediates and displacements of the forms that take them).
pub const TAIL: [u8; 8] = [0x11; 8];

/// The group of an encoding in the baselines: map, opcode and prefix set, e.g. `0F 20 [66 48]`.
pub fn group_name(map: &str, opcode: u8, prefixes: &[u8]) -> String {
    let prefixes: Vec<String> = prefixes.iter().map(|byte| format!("{byte:02X}")).collect();
    format!("{map} {opcode:02X} [{}]", prefixes.join(" "))
}

/// A fetch-only memory that holds one candidate at [`RIP`] (the decoder does not depend on CPL).
struct DecodeMemory<'a> {
    bytes: &'a [u8],
}

impl GuestMemory for DecodeMemory<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(RIP)
            .and_then(|offset| self.bytes.get(offset as usize))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// The decoded length of `bytes` at [`RIP`], `None` when the decoder rejects them.
pub fn decoded_length(bytes: &[u8]) -> Option<usize> {
    let mut memory = DecodeMemory { bytes };
    od_isa::long_decode::decode(&mut memory, RIP, context(PrivilegeLevel::Ring0))
        .ok()
        .map(|instruction| usize::from(instruction.length))
}

/// The decoded instruction of `bytes` at [`RIP`].
pub fn decode(
    bytes: &[u8],
) -> Result<od_isa::long_decode::Instruction, od_isa::long_decode::DecodeError> {
    let mut memory = DecodeMemory { bytes };
    od_isa::long_decode::decode(&mut memory, RIP, context(PrivilegeLevel::Ring0))
}

/// Every encoding of the reduced sweep with its group (the first group that produced it), sorted
/// by bytes and unique.
pub fn encodings_with_groups() -> BTreeMap<Vec<u8>, String> {
    let parts = modrm_parts();
    let mut encodings = BTreeMap::new();
    for (map, map_bytes) in MAPS {
        for opcode in 0..=255u8 {
            for prefixes in PREFIX_SETS {
                for part in &parts {
                    let mut candidate = Vec::with_capacity(32);
                    candidate.extend_from_slice(prefixes);
                    candidate.extend_from_slice(map_bytes);
                    candidate.push(opcode);
                    candidate.extend_from_slice(part);
                    candidate.extend_from_slice(&TAIL);
                    if let Some(length) = decoded_length(&candidate) {
                        candidate.truncate(length);
                        encodings
                            .entry(candidate)
                            .or_insert_with(|| group_name(map, opcode, prefixes));
                    }
                }
            }
        }
    }
    encodings
}

/// The reduced outcome sweep of plan C1-0 item 1: unique byte strings of exactly the instruction
/// length, sorted.
pub fn encodings() -> Vec<Vec<u8>> {
    encodings_with_groups().into_keys().collect()
}

// ---- the sweep memory -----------------------------------------------------------------------------

/// The kind of a logged data access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataAccess {
    Read,
    /// A read with write intent (`read_integer_for_write` / `read_for_write_integer`).
    ReadForWrite,
    Write,
    /// One atomic read-modify-write.
    Atomic,
}

/// One logged data access (fetches are counted, not logged).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Access {
    pub address: u64,
    pub len: usize,
    pub kind: DataAccess,
    pub origin: AccessOrigin,
    pub privilege: PrivilegeLevel,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub bytes: Rc<[u8; 4096]>,
    pub user: bool,
    pub writable: bool,
    pub present: bool,
}

/// Linear = physical 4 KiB pages (plan C1-0 item 2).
#[derive(Clone, Debug, Default)]
pub struct SweepMemory {
    pub pages: BTreeMap<u64, Page>,
    /// A missing page touched by an `Explicit` or `ImplicitStack` data access becomes a user writable
    /// page with [`pattern`] bytes (never for a fetch or a `DescriptorTable` access, never for a page
    /// entered with `present = false`).
    pub on_demand: bool,
    /// The writes in order: address and bytes.
    pub writes: Vec<(u64, Vec<u8>)>,
    /// Every data access in order (successful or not).
    pub accesses: Vec<Access>,
    pub fetches: usize,
    /// ADR-0023 slice C2a: CR0.WP = 0 (Vol. 3A §4.6: a supervisor write to a read-only page is
    /// allowed; a user write still faults). `false` (WP = 1) is the G0 behaviour.
    pub write_protect_off: bool,
    /// ADR-0023 slice C2a (review F1): the bytes of a page mapped from now on are
    /// [`full_address_pattern`] (a hash of the whole address), so that two operands at the same page
    /// offset of different pages read different bytes. `false` ([`pattern`], the low eight address
    /// bits) is the G0 behaviour of the frozen machine.
    pub full_address_pattern: bool,
}

/// The byte of an on-demand page at `address`.
pub const fn pattern(address: u64) -> u8 {
    0x5a ^ (address & 0xff) as u8
}

/// The byte of a page mapped with `full_address_pattern` at `address` (SplitMix64 of all 64 bits).
pub const fn full_address_pattern(address: u64) -> u8 {
    splitmix(address) as u8
}

thread_local! {
    static FULL_PATTERN_PAGES: RefCell<BTreeMap<u64, Rc<[u8; 4096]>>> =
        const { RefCell::new(BTreeMap::new()) };
}

fn pattern_page(page: u64, full: bool) -> Rc<[u8; 4096]> {
    if full {
        // The full-address pages are the same for every run: built once per thread.
        if let Some(bytes) = FULL_PATTERN_PAGES.with(|pages| pages.borrow().get(&page).cloned()) {
            return bytes;
        }
    }
    let mut bytes = [0u8; 4096];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        let address = page + offset as u64;
        *byte = if full {
            full_address_pattern(address)
        } else {
            pattern(address)
        };
    }
    let bytes = Rc::new(bytes);
    if full {
        FULL_PATTERN_PAGES.with(|pages| pages.borrow_mut().insert(page, bytes.clone()));
    }
    bytes
}

impl SweepMemory {
    /// Map `page` (any address in it) with the [`pattern`] contents.
    pub fn map(&mut self, page: u64, user: bool, writable: bool) {
        let page = page & !0xfff;
        self.pages.insert(
            page,
            Page {
                bytes: pattern_page(page, self.full_address_pattern),
                user,
                writable,
                present: true,
            },
        );
    }

    /// Enter `page` as not present: no on-demand mapping for it.
    pub fn absent(&mut self, page: u64) {
        let page = page & !0xfff;
        self.pages.insert(
            page,
            Page {
                bytes: Rc::new([0; 4096]),
                user: true,
                writable: true,
                present: false,
            },
        );
    }

    /// Store bytes without any check or log (fixture setup).
    pub fn put(&mut self, address: u64, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            let address = address.wrapping_add(index as u64);
            let page = self
                .pages
                .get_mut(&(address & !0xfff))
                .expect("mapped page");
            Rc::make_mut(&mut page.bytes)[(address & 0xfff) as usize] = *byte;
        }
    }

    pub fn put_u16(&mut self, address: u64, value: u16) {
        self.put(address, &value.to_le_bytes());
    }

    pub fn put_u64(&mut self, address: u64, value: u64) {
        self.put(address, &value.to_le_bytes());
    }

    /// Read bytes without any check or log (`None` if a page is missing).
    pub fn peek(&self, address: u64, len: usize) -> Option<Vec<u8>> {
        (0..len)
            .map(|index| {
                let address = address.wrapping_add(index as u64);
                self.pages
                    .get(&(address & !0xfff))
                    .filter(|page| page.present)
                    .map(|page| page.bytes[(address & 0xfff) as usize])
            })
            .collect()
    }

    pub fn peek_u64(&self, address: u64) -> Option<u64> {
        self.peek(address, 8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// The number of data reads (any kind except `Write`).
    pub fn data_reads(&self) -> usize {
        self.accesses
            .iter()
            .filter(|access| access.kind != DataAccess::Write)
            .count()
    }

    /// The check of one page of an access: the `#PF` of Vol. 3A "Event 14" (P, W/R, U/S of the page
    /// privilege of the access; CR0.WP = 1 of the machine: a write to a read-only page faults at any
    /// CPL), or Ok. On-demand mapping happens here.
    fn check_page(
        &mut self,
        page: u64,
        fault_address: u64,
        access: AccessKind,
        context: AccessContext,
    ) -> Result<(), MemoryError> {
        if !self.pages.contains_key(&page)
            && self.on_demand
            && access != AccessKind::Fetch
            && context.origin != AccessOrigin::DescriptorTable
        {
            self.map(page, true, true);
        }
        let user = context.page_privilege() == PrivilegeLevel::Ring3;
        let present = match self.pages.get(&page) {
            Some(entry) if entry.present => {
                let write_denied = access == AccessKind::Write
                    && !entry.writable
                    && (user || !self.write_protect_off);
                if !(user && !entry.user) && !write_denied {
                    return Ok(());
                }
                true
            }
            _ => false,
        };
        Err(MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(page_fault_error_code(access, context, present, false, true)),
            linear_address: Some(fault_address),
        }))
    }

    /// Check every page of `[address, address + len)` (the lower page first) before any data moves.
    fn check(
        &mut self,
        address: u64,
        len: usize,
        access: AccessKind,
        context: AccessContext,
    ) -> Result<(), MemoryError> {
        if len == 0 {
            return Ok(());
        }
        let last = address
            .checked_add(len as u64 - 1)
            .ok_or(MemoryError::AddressOverflow {
                address,
                length: len,
            })?;
        let mut page = address & !0xfff;
        loop {
            let fault_address = if page == address & !0xfff {
                address
            } else {
                page
            };
            self.check_page(page, fault_address, access, context)?;
            if page == last & !0xfff {
                return Ok(());
            }
            page += 0x1000;
        }
    }

    fn load(&self, address: u64, out: &mut [u8]) {
        for (index, byte) in out.iter_mut().enumerate() {
            let address = address.wrapping_add(index as u64);
            *byte = self.pages[&(address & !0xfff)].bytes[(address & 0xfff) as usize];
        }
    }

    fn store(&mut self, address: u64, bytes: &[u8]) {
        self.put(address, bytes);
        self.writes.push((address, bytes.to_vec()));
    }

    fn log(&mut self, address: u64, len: usize, kind: DataAccess, context: AccessContext) {
        self.accesses.push(Access {
            address,
            len,
            kind,
            origin: context.origin,
            privilege: context.privilege,
        });
    }

    fn read_checked(
        &mut self,
        address: u64,
        context: AccessContext,
        out: &mut [u8],
        kind: DataAccess,
        access: AccessKind,
    ) -> Result<(), MemoryError> {
        self.log(address, out.len(), kind, context);
        self.check(address, out.len(), access, context)?;
        self.load(address, out);
        Ok(())
    }

    fn integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        kind: DataAccess,
    ) -> Result<u64, MemoryError> {
        let mut bytes = [0u8; 8];
        self.read_checked(
            address,
            context,
            &mut bytes[..size.bytes()],
            kind,
            AccessKind::Write,
        )?;
        Ok(u64::from_le_bytes(bytes))
    }
}

impl GuestMemory for SweepMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetches += 1;
        self.check(address, 1, AccessKind::Fetch, context)?;
        let mut byte = [0u8];
        self.load(address, &mut byte);
        Ok(byte[0])
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.read_checked(
            address,
            context,
            destination,
            DataAccess::Read,
            AccessKind::Read,
        )
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.log(address, source.len(), DataAccess::Write, context);
        self.check(address, source.len(), AccessKind::Write, context)?;
        self.store(address, source);
        Ok(())
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.integer(address, context, size, DataAccess::ReadForWrite)
    }

    fn read_for_write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.integer(address, context, size, DataAccess::ReadForWrite)
    }

    fn preflight_descriptor_accessed_write(
        &mut self,
        descriptor_access_address: u64,
        descriptor_context: AccessContext,
        stack_writes: &[(u64, usize)],
    ) -> Result<(), MemoryError> {
        for &(address, len) in stack_writes {
            self.check(
                address,
                len,
                AccessKind::Write,
                AccessContext {
                    origin: AccessOrigin::ImplicitStack,
                    ..descriptor_context
                },
            )?;
        }
        self.check(
            descriptor_access_address,
            1,
            AccessKind::Write,
            descriptor_context,
        )
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_rmw_integer(address, context, size, &mut |_| replacement)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_rmw_integer(address, context, size, &mut |old| {
            old.wrapping_add(addend) & size.mask()
        })
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let expected = expected & size.mask();
        self.atomic_rmw_integer(address, context, size, &mut |old| {
            if old == expected {
                replacement & size.mask()
            } else {
                old
            }
        })
    }

    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        self.log(address, size.bytes(), DataAccess::Atomic, context);
        self.check(address, size.bytes(), AccessKind::Write, context)?;
        let mut bytes = [0u8; 8];
        self.load(address, &mut bytes[..size.bytes()]);
        let old = u64::from_le_bytes(bytes);
        let new = update(old) & size.mask();
        self.store(address, &new.to_le_bytes()[..size.bytes()]);
        Ok(old)
    }

    fn atomic_rmw_u128(
        &mut self,
        address: u64,
        context: AccessContext,
        update: &mut dyn FnMut(u128) -> u128,
    ) -> Result<u128, MemoryError> {
        self.log(address, 16, DataAccess::Atomic, context);
        self.check(address, 16, AccessKind::Write, context)?;
        let mut bytes = [0u8; 16];
        self.load(address, &mut bytes);
        let old = u128::from_le_bytes(bytes);
        let new = update(old);
        self.store(address, &new.to_le_bytes());
        Ok(old)
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }

    fn fence(&mut self, _: od_isa::FenceKind) -> Result<(), MemoryError> {
        Ok(())
    }
}

// ---- the machine ----------------------------------------------------------------------------------

pub const GDT: u64 = 0x1000;
pub const IDT: u64 = 0x2000;
/// The 64-bit TSS (three supervisor pages: the TSS, then the I/O bitmap up to 0x5068).
pub const TSS: u64 = 0x3000;
pub const IO_MAP_BASE: u16 = 0x68;
pub const TSS_LIMIT: u32 = IO_MAP_BASE as u32 + 0x2000;
/// The supervisor stack page of RSP0 (top `KERNEL_STACK + 0x1000`).
pub const KERNEL_STACK: u64 = 0x6000;
pub const CODE_PAGE: u64 = 0x40_0000;
pub const RIP: u64 = 0x40_0100;
pub const DATA: u64 = 0x50_0000;
pub const STACK_PAGE: u64 = 0x60_0000;
pub const RSP: u64 = STACK_PAGE + 0x1000;
pub const FS_BASE: u64 = 0x70_0000;
pub const GS_BASE: u64 = 0x71_0000;
pub const KERNEL_GS_BASE: u64 = 0x72_0000;
pub const CR0: u64 = 0x8005_0033;
/// DE, PSE, PAE, MCE, PGE, OSFXSR, OSXMMEXCPT.
pub const CR4: u64 = 0x6f8;
pub const CR4_DE: u64 = 1 << 3;
pub const CR3: u64 = 0x1a_a000;
pub const EFER: u64 = 0xd01;
pub const HANDLERS: u64 = 0xffff_f800_1000_0000;
pub const TR_SELECTOR: u16 = 0x40;
/// The ports whose bits the TSS bitmap clears (all widths up to 4 at 0x60 and 8 at 0x3f8).
pub const ALLOWED_PORTS: [core::ops::Range<u16>; 2] = [0x60..0x64, 0x3f8..0x400];

pub const CODE64: u64 = 0x00af_9b00_0000_ffff;
pub const DATA0: u64 = 0x00cf_9300_0000_ffff;
pub const USER_CODE32: u64 = 0x00cf_fb00_0000_ffff;
pub const USER_DATA: u64 = 0x00cf_f300_0000_ffff;
pub const USER_CODE64: u64 = 0x00af_fb00_0000_ffff;
/// GDT 0x50 (selector 0x53) like the Windows one: user data whose limit carries the processor
/// number in bits 19:14 (`lsl eax, 0x53; shr eax, 14`); here processor 3, G = 0.
pub const PROCESSOR_NUMBER: u64 = 3;
pub const PROCESSOR_LIMIT: u64 = (PROCESSOR_NUMBER << 14) | 0x3c00;
pub const USER_PROCESSOR_DATA: u64 =
    0x0040_f300_0000_0000 | (PROCESSOR_LIMIT & 0xffff) | ((PROCESSOR_LIMIT >> 16) << 48);

/// The handler of `vector` in the IDT of the machine.
pub const fn handler(vector: u8) -> u64 {
    HANDLERS + vector as u64 * 0x100
}

/// An IDT gate: interrupt gate 0x8e (DPL 0) or 0xee (DPL 3), selector 0x08, `ist`.
pub fn gate(offset: u64, selector: u16, ist: u8, attributes: u8) -> [u8; 16] {
    let mut gate = [0u8; 16];
    gate[0..2].copy_from_slice(&(offset as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&selector.to_le_bytes());
    gate[4] = ist;
    gate[5] = attributes;
    gate[6..8].copy_from_slice(&((offset >> 16) as u16).to_le_bytes());
    gate[8..12].copy_from_slice(&((offset >> 32) as u32).to_le_bytes());
    gate
}

/// The vectors whose gate has DPL 3 (Windows: `INT3`, `INTO`, `__fastfail`, `INT 2C/2D/2E`).
pub const USER_VECTORS: [u8; 6] = [3, 4, 0x29, 0x2c, 0x2d, 0x2e];

pub const fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

pub const fn privilege(cpl: u8) -> PrivilegeLevel {
    match cpl {
        0 => PrivilegeLevel::Ring0,
        1 => PrivilegeLevel::Ring1,
        2 => PrivilegeLevel::Ring2,
        _ => PrivilegeLevel::Ring3,
    }
}

pub fn config(cpl: u8, budget: u64) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: None,
        context: context(privilege(cpl)),
    }
}

/// The GPR value of register `index` (RSP: the top of the stack page).
pub const fn gpr_value(index: usize) -> u64 {
    if index == Register::Rsp as usize {
        RSP
    } else {
        DATA + 0x800 + 0x40 * index as u64
    }
}

/// The machine of the sweep (plan C1-0 item 3) at `cpl` (0 or 3), `iopl` and `cr4`. On-demand
/// mapping is on. The code page is user and writable; nothing is written at `RIP`.
pub fn machine(cpl: u8, iopl: u8, cr4: u64) -> (LongModeState, SweepMemory) {
    let mut memory = SweepMemory::default();
    memory.on_demand = true;
    memory.map(GDT, false, true);
    memory.map(IDT, false, true);
    for page in 0..3 {
        memory.map(TSS + 0x1000 * page, false, true);
    }
    memory.map(KERNEL_STACK, false, true);
    memory.map(CODE_PAGE, true, true);
    memory.map(DATA, true, true);
    memory.map(STACK_PAGE, true, true);
    for (index, descriptor) in [0, CODE64, DATA0, 0, USER_CODE32, USER_DATA, USER_CODE64, 0]
        .into_iter()
        .enumerate()
    {
        memory.put_u64(GDT + 8 * index as u64, descriptor);
    }
    // 0x40: the busy 64-bit TSS (16 bytes).
    let tss_low = (u64::from(TSS_LIMIT) & 0xffff)
        | ((TSS & 0xff_ffff) << 16)
        | (0x8b << 40)
        | ((u64::from(TSS_LIMIT) >> 16) << 48)
        | (((TSS >> 24) & 0xff) << 56);
    memory.put_u64(GDT + 0x40, tss_low);
    memory.put_u64(GDT + 0x48, TSS >> 32);
    memory.put_u64(GDT + 0x50, USER_PROCESSOR_DATA);
    for vector in 0..=255u8 {
        let attributes = if USER_VECTORS.contains(&vector) {
            0xee
        } else {
            0x8e
        };
        memory.put(
            IDT + 16 * u64::from(vector),
            &gate(handler(vector), 0x08, 0, attributes),
        );
    }
    // TSS: RSP0, no IST, the I/O bitmap at IO_MAP_BASE with the allowed ports clear and the
    // terminating 0xff byte.
    for offset in 0..u64::from(TSS_LIMIT) + 1 {
        memory.put(TSS + offset, &[0]);
    }
    memory.put_u64(TSS + 4, KERNEL_STACK + 0x1000);
    memory.put_u16(TSS + 0x66, IO_MAP_BASE);
    let bitmap = TSS + u64::from(IO_MAP_BASE);
    for byte in 0..0x2000u64 {
        memory.put(bitmap + byte, &[0xff]);
    }
    for range in ALLOWED_PORTS {
        for port in range {
            let address = bitmap + u64::from(port / 8);
            let value = memory.peek(address, 1).unwrap()[0] & !(1 << (port % 8));
            memory.put(address, &[value]);
        }
    }
    memory.put(bitmap + 0x2000, &[0xff]);

    let (cs, cs_access, ss, ss_access) = if cpl == 3 {
        (0x33, 0xfb, 0x2b, 0xf3)
    } else {
        (0x08, 0x9b, 0x10, 0x93)
    };
    let data_cache = DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0xf3,
        flags: 0xc,
    };
    let mut state = LongModeState {
        rip: RIP,
        rflags: 2 | (1 << 9) | (u64::from(iopl & 3) << 12),
        cr0: Some(CR0),
        cr4: Some(cr4),
        cr3: Some(CR3),
        cs_selector: Some(cs),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: cs_access,
            flags: 0xa,
        }),
        ss_selector: Some(ss),
        ss_cache: Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: ss_access,
            flags: 0xc,
        }),
        ds_selector: 0x2b,
        es_selector: 0x2b,
        ds_cache: Some(data_cache),
        es_cache: Some(data_cache),
        fs_selector: 0x53,
        gs_selector: 0x2b,
        fs_cache: Some(FsDescriptorCache {
            limit: PROCESSOR_LIMIT as u32,
            access: 0xf3,
            flags: 0x4,
        }),
        gs_cache: Some(FsDescriptorCache {
            limit: u32::MAX,
            access: 0xf3,
            flags: 0xc,
        }),
        fs_base: FS_BASE,
        gs_base: GS_BASE,
        gdtr: Some(Gdtr {
            base: GDT,
            limit: 0x57,
        }),
        idtr: Some(Idtr {
            base: IDT,
            limit: 0xfff,
        }),
        tr: Some(TaskRegisterCache {
            selector: TR_SELECTOR,
            base: TSS,
            limit: TSS_LIMIT,
            access: 0x8b,
            flags: 0,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    for (index, value) in state.gpr.iter_mut().enumerate() {
        *value = gpr_value(index);
    }
    (state, memory)
}

/// The FP cell of the machine: known ST/FSW/FCW/tags, pointers and selectors, XMM and MXCSR.
pub fn fp_seed() -> GuestFpState {
    let mut x87 = X87State::fixture_seed();
    x87.instruction_selector = Some(0x0033);
    x87.data_selector = Some(0x002b);
    let mut xmm = [[0u8; 16]; 16];
    for (register, bytes) in xmm.iter_mut().enumerate() {
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = ((register * 16 + index) as u8) ^ 0x3c;
        }
    }
    GuestFpState {
        x87,
        sse: SseState { xmm, mxcsr: 0x1f80 },
    }
}

pub static EFER_PROFILE: MsrProfile = MsrProfile::EferReadOnlyV1(EFER);

/// The cells of the set (MSR files, CR8, the FP cell).
pub struct Cells {
    pub efer: Cell<u64>,
    pub star: Cell<u64>,
    pub lstar: Cell<u64>,
    pub cstar: Cell<u64>,
    pub fmask: Cell<u64>,
    pub sysenter_cs: Cell<u64>,
    pub sysenter_esp: Cell<u64>,
    pub sysenter_eip: Cell<u64>,
    pub kernel_gs_base: Cell<u64>,
    pub pat: Cell<u64>,
    pub misc_enable: Cell<u64>,
    pub bios_sign_id: Cell<u64>,
    pub mtrr: Cell<MtrrState>,
    pub mca: Cell<McaState>,
    pub cr8: Cell<u8>,
    pub fp: RefCell<GuestFpState>,
}

impl Cells {
    /// Windows-like values (STAR: SYSCALL CS 0x10, SYSRET CS base 0x23).
    pub fn new() -> Self {
        Self {
            efer: Cell::new(EFER),
            star: Cell::new(0x0023_0010_0000_0000),
            lstar: Cell::new(0xffff_f800_2000_0000),
            cstar: Cell::new(0xffff_f800_2000_1000),
            fmask: Cell::new(0x4700),
            sysenter_cs: Cell::new(0),
            sysenter_esp: Cell::new(0),
            sysenter_eip: Cell::new(0),
            kernel_gs_base: Cell::new(KERNEL_GS_BASE),
            pat: Cell::new(IA32_PAT_RESET),
            misc_enable: Cell::new(IA32_MISC_ENABLE_RESET),
            bios_sign_id: Cell::new(0),
            mtrr: Cell::new(MtrrState::default()),
            mca: Cell::new(McaState::default()),
            cr8: Cell::new(0),
            fp: RefCell::new(fp_seed()),
        }
    }
}

impl Default for Cells {
    fn default() -> Self {
        Self::new()
    }
}

pub static CPUID: CpuidProfile = CpuidProfile::OdX8664V2Windows;

/// The set of the sweep: every member it needs, the Windows profile of the unified loop.
pub fn set(cells: &Cells) -> Cpl0ProfileSet<'_> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        // CPUID: the Windows profile S2-57 (FPU, FXSR, SSE, SSE2, DE, MCE ...).
        .with_cpuid(&CPUID)
        // RDTSC from the source of the run.
        .with_rdtsc()
        // CLI/STI at CPL0 and at CPL3 with IOPL 3.
        .with_cli(&CliProfile::NonPviLong64V1)
        .with_sti(&StiProfile::NonPviLong64V1)
        // HLT at CPL0 (the member must not decide CPL3).
        .with_halt(&HaltProfile::Long64V1)
        // INT3 at CPL0.
        .with_int3()
        // IN/OUT at CPL0 through the test port.
        .with_pio_u8()
        // MOV CR3 retires with a translation-context exit.
        .with_cpu_owned_cr3(MovCr3Long64V1)
        // IRETQ to CPL3 commits; SYSCALL from CPL3.
        .with_user_mode()
        .with_syscall()
        // x87/MMX/SSE on the cell.
        .with_fp_state(&cells.fp)
        // POPF with IF/IOPL at CPL0.
        .with_popf_cpl0()
        // MOV CR8 on its cell (the TPR of the Windows set).
        .with_cr8_tpr(&cells.cr8)
        // The MSR members the Windows member builds on, then the Windows member and its files.
        .with_msr(&EFER_PROFILE)
        .with_msr_efer_write(&cells.efer)
        .with_msr_fs_gs_base()
        .with_msr_apic_base()
        .with_msr_star_write(&cells.star)
        .with_msr_lstar_write(&cells.lstar)
        .with_msr_cstar_write(&cells.cstar)
        .with_msr_syscall_mask_write(&cells.fmask)
        .with_msr_sysenter_cs_write(&cells.sysenter_cs)
        .with_msr_sysenter_stack_write(&cells.sysenter_esp, &cells.sysenter_eip)
        .with_msr_kernel_gs_base_write(&cells.kernel_gs_base)
        .with_msr_windows(&cells.pat, &cells.misc_enable)
        .with_msr_windows_unknown_as_stop()
        .with_msr_bios_sign_id(&cells.bios_sign_id)
        .with_msr_mtrr(&cells.mtrr)
        .with_msr_mca(&cells.mca)
}

/// The test port: reads return `0xa5 ^ port`, every call is logged.
#[derive(Default)]
pub struct SweepPort {
    pub calls: Vec<(char, u16, u32)>,
}

impl PortIo for SweepPort {
    type Error = ();

    fn read_u8(&mut self, port: u16) -> Result<u8, ()> {
        let value = 0xa5 ^ port as u8;
        self.calls.push(('r', port, u32::from(value)));
        Ok(value)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), ()> {
        self.calls.push(('w', port, u32::from(value)));
        Ok(())
    }
}

/// The RDTSC source value of every run.
pub const TSC: u64 = 0x0000_1234_5678_9abc;

/// One run (budget `budget`) of `bytes` at `RIP` on `state`/`memory` with the cells and the set of
/// the sweep: CPL0 through `run_with_cpl0_profile_set_and_pio`, CPL3 through
/// `run_with_cpl3_profile_set`.
pub struct Run {
    pub result: Result<Cpl0ProfileSetResult<()>, Cpl0ProfileSetInputError>,
    pub state: LongModeState,
    pub memory: SweepMemory,
    pub cells: Cells,
    pub port: SweepPort,
}

pub fn run_on(
    state: LongModeState,
    memory: SweepMemory,
    cells: Cells,
    bytes: &[u8],
    budget: u64,
) -> Run {
    run_on_with(state, memory, cells, bytes, budget, set)
}

/// [`run_on`] with the set `make` builds over the cells (ADR-0023 slice C2a: the Linux set of the
/// legacy-path golden, the sweep set without the FP member).
pub fn run_on_with(
    mut state: LongModeState,
    mut memory: SweepMemory,
    cells: Cells,
    bytes: &[u8],
    budget: u64,
    make: impl for<'c> Fn(&'c Cells) -> Cpl0ProfileSet<'c>,
) -> Run {
    memory.put(state.rip, bytes);
    let mut port = SweepPort::default();
    let cpl = state.cs_selector.map_or(0, |selector| selector & 3);
    let result = {
        let set = make(&cells);
        let mut tsc = || TSC;
        if cpl == 3 {
            run_with_cpl3_profile_set(
                &mut state,
                &mut memory,
                config(3, budget),
                od_isa::INTEGER_FLAGS,
                &set,
                Some(&mut tsc),
                || false,
            )
            .map(|result| Cpl0ProfileSetResult {
                exit: match result.exit {
                    od_isa::Cpl0ProfileSetExit::Run(exit) => od_isa::Cpl0ProfileSetExit::Run(exit),
                    other => panic!("the CPL3 entry ended with {other:?}"),
                },
                instructions_executed: result.instructions_executed,
                rflags_defined: result.rflags_defined,
            })
        } else {
            run_with_cpl0_profile_set_and_pio(
                &mut state,
                &mut memory,
                config(0, budget),
                od_isa::INTEGER_FLAGS,
                &set,
                Some(&mut tsc),
                &mut port,
                || false,
            )
        }
    };
    Run {
        result,
        state,
        memory,
        cells,
        port,
    }
}

/// One instruction (budget 1) of `bytes` on `machine(cpl, iopl, cr4)`.
pub fn run_one(bytes: &[u8], cpl: u8, iopl: u8, cr4: u64) -> Run {
    let (state, memory) = machine(cpl, iopl, cr4);
    run_on(state, memory, Cells::new(), bytes, 1)
}

// ---- the outcome ----------------------------------------------------------------------------------

/// The expanded outcome of a run (plan C1-0 item 4): the exit with its whole payload and the retired
/// count, every field of the state (destructured without `..`: a new field does not compile until
/// it is listed), the cells of the set, the FP cell, the write log and the port calls.
pub fn outcome_fields(run: &Run) -> Vec<(&'static str, String)> {
    let LongModeState {
        gpr,
        rip,
        rflags,
        cr0,
        cr4,
        cr2,
        cr3,
        syscall,
        fs_base,
        gs_base,
        kernel_gs_base,
        fs_selector,
        gs_selector,
        ds_selector,
        es_selector,
        cs_selector,
        cs_cache,
        ss_selector,
        ss_cache,
        ds_unusable,
        es_unusable,
        ss_unusable,
        fs_unusable,
        gs_unusable,
        ds_cache,
        es_cache,
        gdtr,
        idtr,
        tr,
        ldtr_null_selector,
        debug_registers,
        fs_cache,
        gs_cache,
        x87,
        sse,
        sti_shadow,
        activity,
        nmi_blocked,
    } = &run.state;
    const GPR: [&str; 16] = [
        "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12",
        "r13", "r14", "r15",
    ];
    let mut fields: Vec<(&'static str, String)> = vec![("exit", format!("{:?}", run.result))];
    for (name, value) in GPR.iter().zip(gpr) {
        fields.push((name, format!("{value:#x}")));
    }
    let cells = &run.cells;
    fields.extend([
        ("rip", format!("{rip:#x}")),
        ("rflags", format!("{rflags:#x}")),
        ("cr0", format!("{cr0:x?}")),
        ("cr2", format!("{cr2:x?}")),
        ("cr3", format!("{cr3:x?}")),
        ("cr4", format!("{cr4:x?}")),
        ("syscall", format!("{syscall:x?}")),
        ("cs", format!("{cs_selector:x?} {cs_cache:x?}")),
        (
            "ss",
            format!("{ss_selector:x?} {ss_cache:x?} {ss_unusable}"),
        ),
        (
            "ds",
            format!("{ds_selector:#x} {ds_cache:x?} {ds_unusable}"),
        ),
        (
            "es",
            format!("{es_selector:#x} {es_cache:x?} {es_unusable}"),
        ),
        (
            "fs",
            format!("{fs_selector:#x} {fs_cache:x?} {fs_unusable} {fs_base:#x}"),
        ),
        (
            "gs",
            format!("{gs_selector:#x} {gs_cache:x?} {gs_unusable} {gs_base:#x}"),
        ),
        ("kernel_gs_base", format!("{kernel_gs_base:x?}")),
        ("gdtr", format!("{gdtr:x?}")),
        ("idtr", format!("{idtr:x?}")),
        ("ldtr", format!("{ldtr_null_selector:x?}")),
        ("tr", format!("{tr:x?}")),
        ("dr", format!("{debug_registers:x?}")),
        (
            "activity",
            format!("{activity:?} {sti_shadow:?} {nmi_blocked}"),
        ),
        ("state.x87", format!("{x87:x?}")),
        ("state.sse", format!("{sse:x?}")),
        ("msr.efer", format!("{:#x}", cells.efer.get())),
        ("msr.star", format!("{:#x}", cells.star.get())),
        ("msr.lstar", format!("{:#x}", cells.lstar.get())),
        ("msr.cstar", format!("{:#x}", cells.cstar.get())),
        ("msr.fmask", format!("{:#x}", cells.fmask.get())),
        (
            "msr.sysenter",
            format!(
                "{:#x} {:#x} {:#x}",
                cells.sysenter_cs.get(),
                cells.sysenter_esp.get(),
                cells.sysenter_eip.get()
            ),
        ),
        (
            "msr.kernel_gs_base",
            format!("{:#x}", cells.kernel_gs_base.get()),
        ),
        (
            "msr.windows",
            format!(
                "{:#x} {:#x} {:#x}",
                cells.pat.get(),
                cells.misc_enable.get(),
                cells.bios_sign_id.get()
            ),
        ),
        ("msr.mtrr", format!("{:x?}", cells.mtrr.get())),
        ("msr.mca", format!("{:x?}", cells.mca.get())),
        ("cr8", format!("{}", cells.cr8.get())),
        ("fp", format!("{:x?}", cells.fp.borrow())),
        ("writes", format!("{:x?}", run.memory.writes)),
        ("port", format!("{:x?}", run.port.calls)),
    ]);
    fields
}

/// FNV-1a 64 over bytes.
pub fn fnv1a(hash: u64, bytes: &[u8]) -> u64 {
    let mut hash = hash;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// The digest of an expanded outcome: FNV-1a 64 over `name=value\n` of every field in order.
pub fn digest_fields(fields: &[(&'static str, String)]) -> u64 {
    let mut hash = FNV_OFFSET;
    for (name, value) in fields {
        hash = fnv1a(hash, name.as_bytes());
        hash = fnv1a(hash, b"=");
        hash = fnv1a(hash, value.as_bytes());
        hash = fnv1a(hash, b"\n");
    }
    hash
}

/// The digest of a run (plan C1-0 item 4; C2a-1 uses the same).
pub fn outcome_digest(run: &Run) -> u64 {
    digest_fields(&outcome_fields(run))
}

/// The fields of `after` that differ from `before`, the exit always (the readable dump).
pub fn changed_fields(
    before: &[(&'static str, String)],
    after: &[(&'static str, String)],
) -> String {
    let mut out = String::new();
    for ((name, old), (_, new)) in before.iter().zip(after) {
        if *name == "exit" || old != new {
            out.push_str(&format!(" {name}={new}"));
        }
    }
    out
}

/// The expanded fields of the initial machine (no run): the base of [`changed_fields`].
pub fn initial_fields(cpl: u8, iopl: u8, cr4: u64) -> Vec<(&'static str, String)> {
    let (state, memory) = machine(cpl, iopl, cr4);
    outcome_fields(&Run {
        result: Err(Cpl0ProfileSetInputError::WrongMode),
        state,
        memory,
        cells: Cells::new(),
        port: SweepPort::default(),
    })
}

/// Run `work` over `items` on every available core with a per-thread state from `init`; results in
/// item order (deterministic).
pub fn parallel_map_with<T: Sync, S, R: Send>(
    items: &[T],
    init: impl Fn() -> S + Sync,
    work: impl Fn(&mut S, &T) -> R + Sync,
) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = items.len().div_ceil(threads).max(1);
    let (init, work) = (&init, &work);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let mut local = init();
                    part.iter()
                        .map(|item| work(&mut local, item))
                        .collect::<Vec<R>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("sweep thread"))
            .collect()
    })
}

/// One run of `bytes` on a clone of `base` (a [`machine`]) with fresh cells.
pub fn run_cloned(base: &(LongModeState, SweepMemory), bytes: &[u8], budget: u64) -> Run {
    run_on(base.0.clone(), base.1.clone(), Cells::new(), bytes, budget)
}

// ---- helpers added after the C1-0 baseline (they do not change the frozen machine and set) --------

/// One CPL3-entry run of the code already at `state.rip` with a caller's set and an optional
/// RDTSC source.
pub fn run_cpl3_with(
    state: &mut LongModeState,
    memory: &mut SweepMemory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
    tsc: Option<u64>,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    let mut source = move || tsc.unwrap_or(0);
    run_with_cpl3_profile_set(
        state,
        memory,
        config(3, budget),
        od_isa::INTEGER_FLAGS,
        set,
        tsc.map(|_| &mut source as &mut dyn FnMut() -> u64),
        || false,
    )
}

/// One CPL0-entry run (with the test port) of the code already at `state.rip` with a caller's set.
pub fn run_cpl0_with(
    state: &mut LongModeState,
    memory: &mut SweepMemory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
    port: &mut SweepPort,
) -> Result<Cpl0ProfileSetResult<()>, Cpl0ProfileSetInputError> {
    let mut source = || TSC;
    run_with_cpl0_profile_set_and_pio(
        state,
        memory,
        config(0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        Some(&mut source),
        port,
        || false,
    )
}

/// The `#GP(0)` exit.
pub const GP0: od_isa::execute::RunExit = od_isa::execute::RunExit::Fault(ArchitecturalFault {
    vector: 13,
    error_code: Some(0),
    linear_address: None,
});

/// The `#UD` exit.
pub const UD: od_isa::execute::RunExit = od_isa::execute::RunExit::Fault(ArchitecturalFault {
    vector: 6,
    error_code: None,
    linear_address: None,
});

/// The `Run` exit of a result (panics on another exit).
pub fn run_exit<E: core::fmt::Debug>(
    result: &Cpl0ProfileSetResult<E>,
) -> &od_isa::execute::RunExit {
    match &result.exit {
        od_isa::Cpl0ProfileSetExit::Run(exit) => exit,
        other => panic!("expected a Run exit, got {other:?}"),
    }
}

/// The `#PF` exit with `error_code` at `address`.
pub fn page_fault(error_code: u32, address: u64) -> od_isa::execute::RunExit {
    od_isa::execute::RunExit::Fault(ArchitecturalFault {
        vector: 14,
        error_code: Some(error_code),
        linear_address: Some(address),
    })
}

thread_local! {
    static MACHINES: RefCell<BTreeMap<(u8, u8, u64), (LongModeState, SweepMemory)>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// [`machine`] through a per-thread cache (a clone; the pages are shared copy-on-write).
pub fn machine_cached(cpl: u8, iopl: u8, cr4: u64) -> (LongModeState, SweepMemory) {
    MACHINES.with(|machines| {
        machines
            .borrow_mut()
            .entry((cpl, iopl, cr4))
            .or_insert_with(|| machine(cpl, iopl, cr4))
            .clone()
    })
}

/// Plan C1-7 item 3: the classification sweep (decode only). The maps × opcodes × 8 prefix sets
/// without REX (none, `66`, `F2`, `F3`, `67`, `64`, `65`, `F0`) × 17 REX choices (none, `40`-`4F`) ×
/// ModRM, with the safe cut-offs: (a) when the decoder did not consume the ModRM byte, the other 255
/// values are not tried for that combination; (b) SIB takes 4 values (`24`, `25`, `E5`, `77`);
/// (c) displacements and immediates are fixed (zeros, then the `0x11` tail). Calls `visit` with each
/// decoded encoding (bytes cut to the instruction length; duplicates possible).
pub fn full_classification_sweep(mut visit: impl FnMut(&[u8], &od_isa::long_decode::Instruction)) {
    const LEGACY: [&[u8]; 8] = [
        &[],
        &[0x66],
        &[0xf2],
        &[0xf3],
        &[0x67],
        &[0x64],
        &[0x65],
        &[0xf0],
    ];
    for (_, map_bytes) in MAPS {
        for opcode in 0..=255u8 {
            full_sweep_opcode(map_bytes, opcode, &LEGACY, &mut visit);
        }
    }
}

/// One map and opcode of [`full_classification_sweep`] (the unit of parallel work).
pub fn full_sweep_opcode(
    map_bytes: &[u8],
    opcode: u8,
    legacy: &[&[u8]],
    visit: &mut impl FnMut(&[u8], &od_isa::long_decode::Instruction),
) {
    for prefixes in legacy {
        for rex in std::iter::once(None).chain((0x40..=0x4fu8).map(Some)) {
            let mut head = prefixes.to_vec();
            head.extend(rex);
            head.extend_from_slice(map_bytes);
            head.push(opcode);
            let modrm_at = head.len();
            for modrm in 0..=255u8 {
                let sibs: &[u8] = if modrm >> 6 != 3 && modrm & 7 == 4 {
                    &[0x24, 0x25, 0xe5, 0x77]
                } else {
                    &[0]
                };
                let mut consumed_modrm = true;
                for &sib in sibs {
                    let mut candidate = head.clone();
                    candidate.push(modrm);
                    if modrm >> 6 != 3 && modrm & 7 == 4 {
                        candidate.push(sib);
                    }
                    candidate.extend_from_slice(&[0; 4]);
                    candidate.extend_from_slice(&TAIL);
                    match decode(&candidate) {
                        Ok(instruction) => {
                            let length = usize::from(instruction.length);
                            if length <= modrm_at {
                                consumed_modrm = false;
                            }
                            visit(&candidate[..length], &instruction);
                        }
                        Err(_) => {
                            // A rejected candidate tells nothing about the ModRM byte: keep going.
                        }
                    }
                    if !consumed_modrm {
                        break;
                    }
                }
                if !consumed_modrm {
                    break;
                }
            }
        }
    }
}

// ---- ADR-0023 slice C2a: the inputs of the legacy-path golden and of the CPL0 <-> CPL3 tests ------

/// The inputs of plan C2a-1 item 3 (`legacy_digest`) and C2a-2.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum InputVariant {
    /// The machine as [`machine`] builds it: `gpr[i] = DATA + 0x800 + 0x40 i`, RFLAGS `0x202`.
    A,
    /// Deterministic "random" GPRs (random offsets in pages of 16..32 MiB; RSP in the stack page,
    /// 8-aligned) and every status flag set.
    B,
    /// A with RCX = 3, RSI and RDI on different pages (strings, `LOOP`).
    C,
    /// A with DF = 1.
    D,
    /// A with the data page not present and no on-demand mapping.
    E,
    /// A with the data page a read-only user page and no on-demand mapping.
    EPrime,
}

pub const INPUTS: [InputVariant; 6] = [
    InputVariant::A,
    InputVariant::B,
    InputVariant::C,
    InputVariant::D,
    InputVariant::E,
    InputVariant::EPrime,
];

/// SplitMix64 of `index` (the deterministic values of input B).
pub const fn splitmix(index: u64) -> u64 {
    let mut z = index.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The status flags CF, PF, AF, ZF, SF, OF.
pub const STATUS_FLAGS: u64 = 0x8d5;

/// [`machine`] at `cpl` (IOPL 0, `cr4`) shaped by `input`.
pub fn input_machine(cpl: u8, cr4: u64, input: InputVariant) -> (LongModeState, SweepMemory) {
    let (mut state, mut memory) = machine_cached(cpl, 0, cr4);
    match input {
        InputVariant::A => {}
        InputVariant::B => {
            for (index, value) in state.gpr.iter_mut().enumerate() {
                let random = splitmix(index as u64);
                *value = if index == Register::Rsp as usize {
                    STACK_PAGE + 0x800 + (random & 0x7f8)
                } else {
                    0x0100_0000 + (random & 0x00ff_ffff)
                };
            }
            state.rflags |= STATUS_FLAGS;
        }
        InputVariant::C => {
            state.gpr[Register::Rcx as usize] = 3;
            state.gpr[Register::Rsi as usize] = DATA + 0x100;
            state.gpr[Register::Rdi as usize] = DATA + 0x1000 + 0x140;
        }
        InputVariant::D => state.rflags |= 1 << 10,
        InputVariant::E => {
            memory.on_demand = false;
            memory.absent(DATA);
        }
        InputVariant::EPrime => {
            memory.on_demand = false;
            memory.map(DATA, true, false);
        }
    }
    (state, memory)
}

/// The signature of a form (plan C2a-1 item 2): the prefix bytes, the map and opcode, ModRM
/// mod/reg/rm, a SIB byte, the displacement and immediate sizes (`None` if the length decoder
/// does not measure the bytes).
pub fn form_signature(bytes: &[u8]) -> Option<String> {
    let layout = od_isa::length::long64_layout(bytes).ok()?;
    let mut at = 0;
    while matches!(
        bytes.get(at),
        Some(
            0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x2e | 0x36 | 0x3e | 0x26 | 0x64 | 0x65 | 0x40
                ..=0x4f
        )
    ) {
        at += 1;
    }
    let prefixes: String = bytes[..at]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let rest_start = |end: usize| -> usize {
        // The opcode bytes of a form without ModRM: the map bytes (`0F`, `0F 38`/`0F 3A`), then
        // one opcode byte; an immediate follows.
        let mut opcode = at;
        if bytes.get(opcode) == Some(&0x0f) {
            opcode += 1;
            if matches!(bytes.get(opcode), Some(0x38 | 0x3a)) {
                opcode += 1;
            }
        }
        (opcode + 1).min(end)
    };
    let (opcode_bytes_end, modrm) = match layout.modrm {
        Some(offset) => (offset, Some(bytes[offset])),
        None => (rest_start(layout.length), None),
    };
    let opcode: String = bytes[at..opcode_bytes_end]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut text = format!("p{prefixes}|o{opcode}");
    let mut operand_start = opcode_bytes_end;
    if let Some(modrm) = modrm {
        let (mode, reg, rm) = (modrm >> 6, (modrm >> 3) & 7, modrm & 7);
        let sib = mode != 3 && rm == 4;
        let displacement = match (mode, rm) {
            (0, 5) => 4,
            (0, 4)
                if bytes
                    .get(opcode_bytes_end + 1)
                    .is_some_and(|sib| sib & 7 == 5) =>
            {
                4
            }
            (1, _) => 1,
            (2, _) => 4,
            _ => 0,
        };
        operand_start = opcode_bytes_end + 1 + usize::from(sib) + displacement;
        text.push_str(&format!(
            "|m{mode}r{reg}m{rm}|s{}|d{displacement}",
            u8::from(sib)
        ));
    }
    text.push_str(&format!(
        "|i{}",
        layout.length.saturating_sub(operand_start)
    ));
    Some(text)
}

/// The CPUID member of the Linux recipes S6-21/S6-22 (`--cpuid-profile stage4a-apic`).
pub static LINUX_CPUID: CpuidProfile = CpuidProfile::X8664V1Stage4AApic;
/// The CR4 of the Linux recipe S6-21 (`stop.cr4` of its record: PSE, PAE, PGE, OSFXSR, OSXMMEXCPT;
/// stage4a-apic declares neither DE nor MCE, so [`CR4`] is outside the admission of the set).
pub const LINUX_CR4: u64 = 0x6b0;
/// The read-only EFER profile of the Linux recipe (`msr:EferReadOnlyV1(3328)`).
pub static LINUX_EFER_PROFILE: MsrProfile = MsrProfile::EferReadOnlyV1(0xd00);

/// The set of the Linux recipe S6-21 (`stop.set_members` of `work/s7-32/t/linux-apic/boot.json`
/// without `pci_config`, `irq` and `clock_owner`, which bind devices of the board) over the sweep
/// cells; the FP member only with `fp_member` (plan C2a-1 item 3).
pub fn linux_set(cells: &Cells, fp_member: bool) -> Cpl0ProfileSet<'_> {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(&CliProfile::NonPviLong64V1)
        .with_cpu_owned_cr3(MovCr3Long64V1)
        .with_cpuid(&LINUX_CPUID)
        .with_rdtsc()
        .with_msr(&LINUX_EFER_PROFILE)
        .with_msr_efer_write(&cells.efer)
        .with_msr_fs_gs_base()
        .with_msr_star_write(&cells.star)
        .with_msr_lstar_write(&cells.lstar)
        .with_msr_cstar_write(&cells.cstar)
        .with_msr_sysenter_cs_write(&cells.sysenter_cs)
        .with_msr_sysenter_stack_write(&cells.sysenter_esp, &cells.sysenter_eip)
        .with_msr_syscall_mask_write(&cells.fmask)
        .with_msr_kernel_gs_base_write(&cells.kernel_gs_base)
        .with_msr_tdx_keyid_partitioning_read()
        .with_msr_apic_base_for_vcpu(true);
    let set = if fp_member {
        set.with_fp_state(&cells.fp)
    } else {
        set
    };
    set.with_int3()
        .with_halt(&HaltProfile::Long64V1)
        .with_pio_u8()
        .with_sti(&StiProfile::NonPviLong64V1)
        .with_popf_cpl0()
        .with_user_mode()
        .with_syscall()
}

/// The hex text of bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Bytes from hex text.
pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

// ---- ADR-0023 slice C2a-2: the CPL0 <-> CPL3 comparison of one encoding (shared by
// `c2a_cpl3_neutral_differential.rs` and the rewritten refusal tests of `cpl3_profile_set.rs`) ------

use od_isa::Cpl0ProfileSetExit;
use od_isa::execute::RunExit;

/// The FP cell of an input (the inputs of C2a-1 keep the cell of the sweep).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FpCell {
    /// `cpl3_sweep::fp_seed`.
    Seed,
    /// The x87 half after FNINIT (every register empty, FCS/FDS 0), the SSE half of the seed.
    EmptyX87,
    /// Two values on the x87 stack (TOP = 6: ST0 = 1.0, ST1 = 2.0), the SSE half of the seed.
    TwoX87,
    /// [`FpCell::EmptyX87`] with a valid 32-bit FNSAVE image (FCW `0x037F`, FTW `0xFFFF`, the rest 0)
    /// at RAX: the source FRSTOR and FLDENV `[rax]` need.
    ImageAtRax,
}

/// The 108-byte FNSAVE image of 32-bit protected mode (Vol. 1 Figure 8-9) of an x87 after FNINIT.
pub fn fnsave_image() -> [u8; 108] {
    let mut image = [0u8; 108];
    image[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
    image[8..10].copy_from_slice(&0xffffu16.to_le_bytes());
    image
}

/// The inputs of the comparison: A-D, E and E' with the cell of the sweep, A with the other cells.
pub const DIFF_INPUTS: [(InputVariant, FpCell); 9] = [
    (InputVariant::A, FpCell::Seed),
    (InputVariant::B, FpCell::Seed),
    (InputVariant::C, FpCell::Seed),
    (InputVariant::D, FpCell::Seed),
    (InputVariant::A, FpCell::EmptyX87),
    (InputVariant::A, FpCell::TwoX87),
    (InputVariant::A, FpCell::ImageAtRax),
    // Review F2: the data page not present and read-only (the rows (a) and (b) of `Allowed`).
    (InputVariant::E, FpCell::Seed),
    (InputVariant::EPrime, FpCell::Seed),
];

/// ADR-0023 slice C2a (review F2): the literal list of the forms whose RMW destination the CPL3 entry
/// reads with write intent — the list of `cpl3_write_intent_rmw_destination` (`execute.rs`), written out
/// here by its table: the primary map, a memory destination with 64-bit addressing, no segment, `67`,
/// `LOCK`/`REP`, `66` exactly for a word, and (opcode, ModRM.reg, operand size):
/// `83 /0` W/D/Q, `83 /5` D, `01` D/Q, `29` D/Q, `FE /0` B, `FF /0` D/Q, `FF /1` D, `80 /4` B, `20` B,
/// `81`/`83 /1,/4` D, `83 /1,/4` Q, `81`/`83 /4` W, `81 /6` D, `09` Q, `D0 /7` B. The test
/// `write_intent_forms_are_exactly_the_literal_list` checks it against the outcome on input E.
pub fn cpl3_write_intent_form(bytes: &[u8]) -> bool {
    use od_isa::long_decode::{OpcodeMap, Operand};
    let Ok(instruction) = decode(bytes) else {
        return false;
    };
    let Ok(layout) = od_isa::length::long64_layout(bytes) else {
        return false;
    };
    let Some(modrm_at) = layout.modrm else {
        return false;
    };
    let modrm = bytes[modrm_at];
    let prefixes = instruction.prefixes;
    let size = instruction.operand_size;
    if instruction.opcode.map != OpcodeMap::Primary
        || modrm >> 6 == 3
        || prefixes.segment.is_some()
        || prefixes.address_override
        || prefixes.group1.is_some()
        || prefixes.operand_override != (size == OperandSize::Word)
    {
        return false;
    }
    let memory_size = match instruction.operation {
        od_isa::long_decode::Operation::Arithmetic {
            dst: Operand::Memory(m),
            ..
        }
        | od_isa::long_decode::Operation::Logic {
            dst: Operand::Memory(m),
            ..
        }
        | od_isa::long_decode::Operation::Shift {
            dst: Operand::Memory(m),
            ..
        } => m,
        _ => return false,
    };
    if memory_size.address_size != OperandSize::Qword || memory_size.size != size {
        return false;
    }
    use OperandSize::{Byte as B, Dword as D, Qword as Q, Word as W};
    let reg = (modrm >> 3) & 7;
    match (instruction.opcode.byte, reg, size) {
        (0x83, 0, W | D | Q) | (0x83, 5, D) => true,
        (0x01 | 0x29, _, D | Q) => true,
        (0xfe, 0, B) | (0xff, 0, D | Q) | (0xff, 1, D) => true,
        (0x80, 4, B) | (0x20, _, B) => true,
        (0x81 | 0x83, 1 | 4, D) | (0x83, 1 | 4, Q) | (0x81 | 0x83, 4, W) | (0x81, 6, D) => true,
        (0x09, _, Q) => true,
        (0xd0, 7, B) => true,
        _ => false,
    }
}

pub fn cells(fp: FpCell) -> Cells {
    let cells = Cells::new();
    if fp != FpCell::Seed {
        let mut cell = cells.fp.borrow_mut();
        // FCS/FDS known and 0 (the bounded x87 profile of the bare run refuses other selectors).
        cell.x87 = od_isa::execute::GuestFpState::initial().x87;
        if fp == FpCell::TwoX87 {
            cell.x87.registers[6] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
            cell.x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40];
            cell.x87.status_word = 6 << 11;
            cell.x87.tag_word = 0x0fff;
        }
    }
    cells
}

/// The literal list of allowed differences (plan C2a-2 item 2).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Allowed {
    /// (a) The U bit of the error code of a `#PF` (U = 1 at CPL3, Vol. 3A "Event 14").
    PageFaultUserBit,
    /// (b) The `#PF` of the RMW destination of the forms the old allow-list read with write intent
    /// (`cpl3_write_intent_rmw_destination`, plan question V3): W = 1 at CPL3 against W = 0 at CPL0.
    WriteIntentRmw,
    /// (c) The x87 code selector FCS (`instruction_selector = Some(cs)` of the x87 executors): the
    /// field of the cell; bytes [16..18) of a 32-bit FNSTENV/FNSAVE image; bytes [12..14) of a
    /// legacy FXSAVE image (without REX.W).
    X87CodeSelector,
    /// (d) `MovSregStore` of CS or SS: the stored value is the selector of the own CPL.
    StoredCsOrSs,
    /// (f) The forms of `cpl3_rep_string` (`F3 A4`, `F3 A5`, `F3 AA`, `F3 AB`, `F3 48 AB`): until C2b
    /// the CPL3 entry charges one budget unit per iteration; RCX = min(RCX, 64) on both sides, the
    /// calls repeat until the instruction retires, faults or [`REP_CALL_LIMIT`] calls; the retired
    /// count and the number of calls are not compared (ADR-0023 decision 3, slice C2b).
    RepStringBudget,
}
// (e) `#AC` does not arise: RFLAGS.AC = 0 on every input (checked by `c1_alignment_check.rs`).

/// The forms of `cpl3_rep_string` (plan C2a-2 item 2 (e)).
pub const REP_STRING_FORMS: [&[u8]; 5] = [
    &[0xf3, 0xa4],
    &[0xf3, 0xa5],
    &[0xf3, 0xaa],
    &[0xf3, 0xab],
    &[0xf3, 0x48, 0xab],
];
/// The literal N of plan C2a-2 (e): reaching it fails the test.
pub const REP_CALL_LIMIT: usize = 4096;
pub fn operation_name(bytes: &[u8]) -> String {
    let instruction = decode(bytes).expect("decodes");
    format!("{:?}", instruction.operation)
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_owned()
}
pub fn is_rep_string_form(bytes: &[u8]) -> bool {
    REP_STRING_FORMS.contains(&bytes)
}

thread_local! {
    static SIDE_MACHINES: RefCell<BTreeMap<(u8, InputVariant, bool), (LongModeState, SweepMemory)>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// The machine of one side before the run (built once per thread and cloned).
fn side_machine(cpl: u8, input: InputVariant, image: bool) -> (LongModeState, SweepMemory) {
    if let Some(machine) =
        SIDE_MACHINES.with(|machines| machines.borrow().get(&(cpl, input, image)).cloned())
    {
        return machine;
    }
    let (state, mut memory) = input_machine(cpl, CR4, input);
    // Plan C2a-2 item 1: every page is a user page, so that CPL0 and CPL3 reach the same pages (the
    // descriptor tables, the TSS and the kernel stack of the machine are not read by a `Neutral`
    // form; a data operand that lands on them is a user access here). The U/S rule is the subject
    // of `c2a_cpl3_page_rights.rs`.
    for page in memory.pages.values_mut() {
        page.user = true;
    }
    // Review F1: every pattern page (and every page mapped on demand from now on) holds the bytes of
    // its full address, so that operands at the same offset of different pages (FS against GS, a
    // wrong base) read different values.
    memory.full_address_pattern = true;
    for (address, page) in memory.pages.iter_mut() {
        if page.present && *page.bytes == *pattern_page(*address, false) {
            page.bytes = pattern_page(*address, true);
        }
    }
    if image {
        memory.put(state.gpr[Register::Rax as usize], &fnsave_image());
    }
    SIDE_MACHINES.with(|machines| {
        machines
            .borrow_mut()
            .insert((cpl, input, image), (state.clone(), memory.clone()))
    });
    (state, memory)
}

/// One side: the run of `bytes` at `cpl` on `input` (the REP forms of (f) repeat).
pub fn side(bytes: &[u8], cpl: u8, (input, fp): (InputVariant, FpCell)) -> Run {
    let (mut state, memory) = side_machine(cpl, input, fp == FpCell::ImageAtRax);
    if !is_rep_string_form(bytes) {
        return run_on(state, memory, cells(fp), bytes, 1);
    }
    let rcx = &mut state.gpr[Register::Rcx as usize];
    *rcx = (*rcx).min(64);
    let start = state.rip;
    let mut run = run_on(state, memory, cells(fp), bytes, 1);
    for _ in 1..REP_CALL_LIMIT {
        let done = run.state.rip != start
            || !matches!(
                &run.result,
                Ok(result) if matches!(
                    result.exit,
                    Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                )
            );
        if done {
            return run;
        }
        run = run_on(run.state, run.memory, run.cells, &[], 1);
    }
    panic!("{}: no retire after {REP_CALL_LIMIT} calls", hex(bytes));
}

/// The fields of one side, normalized by the allowed differences that apply to the pair.
pub fn normalized(
    run: &Run,
    bytes: &[u8],
    name: &str,
    allowed: &BTreeSet<Allowed>,
) -> Vec<(&'static str, String)> {
    let mut fields = outcome_fields(run);
    fields.retain(|(field, _)| !matches!(*field, "cs" | "ss"));
    for (field, value) in fields.iter_mut() {
        match *field {
            "exit" => {
                let mut result = run.result.clone();
                if let Ok(result) = result.as_mut() {
                    if let Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) = &mut result.exit
                        && fault.vector == 14
                        && let Some(code) = fault.error_code.as_mut()
                    {
                        if allowed.contains(&Allowed::PageFaultUserBit) {
                            *code &= !4;
                        }
                        if allowed.contains(&Allowed::WriteIntentRmw) {
                            *code &= !2;
                        }
                    }
                    if allowed.contains(&Allowed::RepStringBudget) {
                        result.instructions_executed = 0;
                    }
                }
                *value = format!("{result:?}");
            }
            "fp" if allowed.contains(&Allowed::X87CodeSelector) => {
                let mut fp = run.cells.fp.borrow().clone();
                fp.x87.instruction_selector = None;
                *value = format!("{fp:x?}");
            }
            "writes" => {
                let mut writes = run.memory.writes.clone();
                let start = writes.iter().map(|(address, _)| *address).min();
                for (address, data) in writes.iter_mut() {
                    for (index, byte) in data.iter_mut().enumerate() {
                        let at = *address + index as u64;
                        if masked_byte(at, start, bytes, name, allowed) {
                            *byte = 0;
                        }
                    }
                }
                if allowed.contains(&Allowed::RepStringBudget) {
                    // The per-iteration calls of the CPL3 entry write the same bytes in the same
                    // order; the log is compared as the final bytes (pages below).
                    writes.clear();
                }
                *value = format!("{writes:x?}");
            }
            _ => {}
        }
        // (d): only the destination register of `MOV r, CS/SS` (a memory destination: `masked_byte`).
        if allowed.contains(&Allowed::StoredCsOrSs)
            && let Some(index) = GPR_FIELDS.iter().position(|gpr| gpr == field)
            && let Ok(instruction) = decode(bytes)
            && let od_isa::long_decode::Operation::MovSregStore {
                dst: od_isa::long_decode::Operand::Register(view),
                ..
            } = instruction.operation
            && view.register() as usize == index
        {
            let gpr = run.state.gpr[index];
            *value = format!("{:#x}", gpr & !0xffff);
        }
    }
    fields
}

/// The pages that differ between the two sides (masked like the write log); untouched pages share
/// their bytes with the machine (copy on write) and are skipped without a comparison.
pub fn page_differences(
    cpl0: &Run,
    cpl3: &Run,
    bytes: &[u8],
    name: &str,
    allowed: &BTreeSet<Allowed>,
) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let keys: BTreeSet<&u64> = cpl0
        .memory
        .pages
        .keys()
        .chain(cpl3.memory.pages.keys())
        .collect();
    let start0 = cpl0.memory.writes.iter().map(|(address, _)| *address).min();
    let start3 = cpl3.memory.writes.iter().map(|(address, _)| *address).min();
    for page in keys {
        let (left, right) = (cpl0.memory.pages.get(page), cpl3.memory.pages.get(page));
        let same = match (left, right) {
            (Some(left), Some(right)) => {
                (left.user, left.writable, left.present)
                    == (right.user, right.writable, right.present)
                    && (std::rc::Rc::ptr_eq(&left.bytes, &right.bytes)
                        || (0..4096).all(|offset| {
                            let at = page + offset as u64;
                            left.bytes[offset] == right.bytes[offset]
                                || masked_byte(at, start0, bytes, name, allowed)
                                    && masked_byte(at, start3, bytes, name, allowed)
                        }))
            }
            // A page that one side created on demand (a data access to a missing page) and did not
            // change is the missing page of the other side: the mapping is the fixture's, not
            // architectural state.
            (Some(only), None) | (None, Some(only)) => {
                only.user
                    && only.writable
                    && only.present
                    && (0..4096).all(|offset| {
                        only.bytes[offset] == full_address_pattern(page + offset as u64)
                    })
            }
            _ => false,
        };
        if !same {
            let describe = |page: Option<&Page>| {
                page.map_or("absent".to_owned(), |page| {
                    format!(
                        "{} {} {} {:016x}",
                        page.user,
                        page.writable,
                        page.present,
                        fnv1a(FNV_OFFSET, &page.bytes[..])
                    )
                })
            };
            out.push((format!("page {page:#x}"), describe(left), describe(right)));
        }
    }
    out
}

pub const GPR_FIELDS: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];

/// Whether the byte at `address` is masked: the FCS bytes of an x87 image (c), the stored selector
/// of (d). `start` is the lowest written address (the start of the image or of the operand).
pub fn masked_byte(
    address: u64,
    start: Option<u64>,
    bytes: &[u8],
    name: &str,
    allowed: &BTreeSet<Allowed>,
) -> bool {
    let Some(start) = start else { return false };
    let offset = address.wrapping_sub(start);
    if allowed.contains(&Allowed::X87CodeSelector) {
        let operand_override =
            decode(bytes).is_ok_and(|instruction| instruction.prefixes.operand_override);
        if (name.starts_with("Fnstenv") || name.starts_with("Fnsave")) && !operand_override {
            return (16..18).contains(&offset);
        }
        if name.starts_with("Fxsave") && !rex_w(bytes) {
            return (12..14).contains(&offset);
        }
    }
    allowed.contains(&Allowed::StoredCsOrSs)
        && offset < 2
        && decode(bytes).is_ok_and(|instruction| {
            matches!(
                instruction.operation,
                od_isa::long_decode::Operation::MovSregStore {
                    dst: od_isa::long_decode::Operand::Memory(_),
                    ..
                }
            )
        })
}

pub fn rex_w(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .take_while(|byte| {
            matches!(
                byte,
                0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x40
                    ..=0x4f
            )
        })
        .any(|byte| (0x48..=0x4f).contains(byte))
}

/// The allowed differences that apply to a pair (from the form and the two runs).
pub fn applicable(bytes: &[u8], name: &str, cpl0: &Run, cpl3: &Run) -> BTreeSet<Allowed> {
    let mut allowed = BTreeSet::new();
    let page_fault = |run: &Run| match &run.result {
        Ok(result) => match &result.exit {
            Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) if fault.vector == 14 => {
                Some((fault.error_code.unwrap_or(0), fault.linear_address))
            }
            _ => None,
        },
        Err(_) => None,
    };
    if let (Some((code0, address0)), Some((code3, address3))) = (page_fault(cpl0), page_fault(cpl3))
        && address0 == address3
    {
        if code0 & 4 == 0 && code3 & 4 != 0 {
            allowed.insert(Allowed::PageFaultUserBit);
        }
    }
    // (b): the forms of the literal list (review F2: by the form, not by the behaviour) read their
    // destination with write intent at CPL3: W = 1 of a #PF, `ReadForWrite` in the access log.
    if cpl3_write_intent_form(bytes) {
        allowed.insert(Allowed::WriteIntentRmw);
    }
    let x87 = name.starts_with('F') && !name.starts_with("Fxrstor") || name.starts_with("Fxsave");
    if x87 {
        allowed.insert(Allowed::X87CodeSelector);
    }
    if name == "MovSregStore"
        && let Ok(instruction) = decode(bytes)
        && let od_isa::long_decode::Operation::MovSregStore { sreg, .. } = instruction.operation
        && matches!(sreg, 1 | 2)
    {
        allowed.insert(Allowed::StoredCsOrSs);
    }
    if is_rep_string_form(bytes) {
        allowed.insert(Allowed::RepStringBudget);
    }
    allowed
}

/// The differences of one encoding on one input: (field, CPL0 value, CPL3 value).
pub fn differences(
    bytes: &[u8],
    input: (InputVariant, FpCell),
) -> (Vec<(String, String, String)>, Run) {
    let name = operation_name(bytes);
    let cpl0 = side(bytes, 0, input);
    let cpl3 = side(bytes, 3, input);
    let allowed = applicable(bytes, &name, &cpl0, &cpl3);
    let a = normalized(&cpl0, bytes, &name, &allowed);
    let b = normalized(&cpl3, bytes, &name, &allowed);
    let mut out = Vec::new();
    for ((field, left), (_, right)) in a.iter().zip(&b) {
        if left != right {
            out.push(((*field).into(), left.clone(), right.clone()));
        }
    }
    out.extend(page_differences(&cpl0, &cpl3, bytes, &name, &allowed));
    // Review F1: the access logs of both sides (kind, address, length, origin; the privilege differs
    // by construction), normalized by the same literal list: (b) a write-intent read of CPL3 is the
    // read of CPL0; (f) the per-iteration calls of `cpl3_rep_string` are compared by their final bytes.
    if !allowed.contains(&Allowed::RepStringBudget) {
        // (b): a read (CPL0) or a write-intent read (CPL3) of the destination, with the write of the
        // same operand right after it if there is one, is one RMW access (a form of the list has no
        // other memory operand; its read faults first at CPL0, its write check first at CPL3).
        let log = |run: &Run, intent: bool| -> Vec<(&'static str, u64, usize, AccessOrigin)> {
            let accesses = &run.memory.accesses;
            let mut out = Vec::new();
            let mut index = 0;
            while index < accesses.len() {
                let access = accesses[index];
                let key = |kind| (kind, access.address, access.len, access.origin);
                let next_writes_it = accesses.get(index + 1).is_some_and(|next| {
                    next.kind == DataAccess::Write
                        && (next.address, next.len) == (access.address, access.len)
                });
                if intent && matches!(access.kind, DataAccess::Read | DataAccess::ReadForWrite) {
                    out.push(key("rmw"));
                    index += if next_writes_it { 2 } else { 1 };
                    continue;
                }
                out.push(key(match access.kind {
                    DataAccess::Read => "read",
                    DataAccess::ReadForWrite => "read-for-write",
                    DataAccess::Write => "write",
                    DataAccess::Atomic => "atomic",
                }));
                index += 1;
            }
            out
        };
        let intent = allowed.contains(&Allowed::WriteIntentRmw);
        let (left, right) = (log(&cpl0, intent), log(&cpl3, intent));
        if left != right {
            out.push((
                "accesses".into(),
                format!("{left:x?}"),
                format!("{right:x?}"),
            ));
        }
    }
    // Review F4: every explicit and stack access of the CPL3 side has the privilege of CPL3.
    if let Some(access) = cpl3.memory.accesses.iter().find(|access| {
        matches!(
            access.origin,
            AccessOrigin::Explicit | AccessOrigin::ImplicitStack
        ) && access.privilege != PrivilegeLevel::Ring3
    }) {
        out.push((
            "access privilege".into(),
            String::new(),
            format!("{access:x?}"),
        ));
    }
    // Review F2: a form of the literal list whose destination read faults at CPL0 (W = 0) faults at
    // CPL3 with W = 1 (the list is not larger than the behaviour of the entry).
    if allowed.contains(&Allowed::WriteIntentRmw) {
        let fault = |run: &Run| match &run.result {
            Ok(result) => match &result.exit {
                Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) if fault.vector == 14 => {
                    fault.error_code
                }
                _ => None,
            },
            Err(_) => None,
        };
        if let (Some(code0), Some(code3)) = (fault(&cpl0), fault(&cpl3))
            && code0 & 2 == 0
            && code3 & 2 == 0
        {
            out.push((
                "write intent".into(),
                format!("{code0:#x}"),
                format!("{code3:#x}"),
            ));
        }
    }
    (out, cpl0)
}

pub fn unsupported(run: &Run) -> bool {
    matches!(
        &run.result,
        Ok(result) if matches!(result.exit, Cpl0ProfileSetExit::Run(RunExit::Unsupported { .. }))
    )
}
