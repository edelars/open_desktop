//! Experiment (branch `exp/turbo-jit`): the turbo loop. A JIT from guest x86-64 to host AArch64 for
//! the hot instructions of the unified boot loop, with an inline software TLB for RAM accesses,
//! guest arithmetic flags kept in the host NZCV register (C inverted: ARM "no borrow" sense),
//! PF/AF in two host registers, and a dispatcher that falls back to the general path of the set
//! loop for everything it does not translate. Not reviewed, not a product decision.
//!
//! Host register convention inside generated code:
//! - x19: `&mut LongModeState` (GPRs, RIP read/written at fixed offsets)
//! - x20: remaining instruction budget of the run (signed)
//! - x21: `&mut TurboCtx`
//! - x22: read TLB, x23: write TLB, x28: lookup table
//! - x24: definedness of the six arithmetic flags (`rflags_defined` bits)
//! - x25: NZCV save around helper calls
//! - x26: the result whose low byte gives PF (PF = even parity)
//! - x27: bit 4 gives AF
//! - NZCV: N = SF, Z = ZF, C = !CF, V = OF

#![allow(dead_code)]
#![allow(clippy::all)]

mod jit;
mod profile;
#[cfg(test)]
mod tests;

use crate::page_walk::{Long64ProbeConfig, RamAccessOperation, TrackedAccessRequest};
use crate::{GuestLinearAddress, GuestPhysicalAddress, PhysicalMemory};
use od_isa::LongModeState;
use od_isa::memory::{AccessContext, AccessKind, PrivilegeLevel};
use od_jit::exec_arena::ExecutableArena;
use profile::{Refusal, SlowProfile, SlowReason};
use std::collections::HashMap as StdHashMap;
use std::collections::HashSet as StdHashSet;

/// A multiply-rotate hasher for integer keys (the maps here are keyed by RIPs and pages).
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher(u64);

impl std::hash::Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }
    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

type Fx = std::hash::BuildHasherDefault<FxHasher>;
pub(crate) type HashMap<K, V> = StdHashMap<K, V, Fx>;
pub(crate) type HashSet<K> = StdHashSet<K, Fx>;

pub(crate) const TLB_BITS: u32 = 10;
pub(crate) const TLB_SIZE: usize = 1 << TLB_BITS;
pub(crate) const LOOKUP_BITS: u32 = 13;
pub(crate) const LOOKUP_SIZE: usize = 1 << LOOKUP_BITS;
pub(crate) const MAX_BLOCK: usize = 64;

/// Exit reasons of generated code (x0 at `exit_common`).
pub(crate) const EXIT_BUDGET: u64 = 1;
pub(crate) const EXIT_LOOKUP: u64 = 2;
pub(crate) const EXIT_SLOW: u64 = 3;
pub(crate) const EXIT_RDTSC: u64 = 4;
/// A REP string instruction ran `REP_ITERATION_WORK_LIMIT` iterations and did not retire.
pub(crate) const EXIT_REP_PARTIAL: u64 = 6;
/// An RDTSC retired and its boundary belongs to the set loop (no fast transition, or an
/// interrupt to deliver there).
pub(crate) const EXIT_BOUNDARY: u64 = 7;

/// Memory helper op bits.
pub(crate) const OP_WRITE: u64 = 0x10;
/// The locked walk of a read-modify-write: fill the TLBs, no data access (the instruction runs
/// again).
pub(crate) const OP_ATOMIC: u64 = 0x20;
/// S7-32: the TLB entry of a 16-byte access (the SSE templates): the walk of the general path
/// (accessed/dirty bits) and the fill, no data access (the check runs again). The size bits are 0.
pub(crate) const OP_FILL128: u64 = 0x40;
/// S7-32: a plain load or store whose operand may be a device register: an access the RAM path
/// refuses as not RAM goes to the set loop's MMIO dispatch, as the general step's would, and the
/// helper answers status 2 (done through a device: the template finishes the instruction and the
/// run ends at the boundary after it, where the loop asks the interrupt source again).
pub(crate) const OP_MMIO: u64 = 0x80;

const ARITH: u64 = od_isa::ARITHMETIC_FLAGS;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct TlbEntry {
    /// Linear page number (address >> 12); `u64::MAX` when empty.
    pub tag: u64,
    /// host address = guest linear address + addend.
    pub addend: u64,
    /// The frame of the table that holds the leaf entry of this translation.
    pub leaf: u64,
    pub pad: u64,
}

const EMPTY_TLB: TlbEntry = TlbEntry {
    tag: u64::MAX,
    addend: 0,
    leaf: u64::MAX,
    pad: 0,
};

/// What the general path's adapter reads of the active translation context.
pub(crate) struct TlbView {
    pub rtlb: *const TlbEntry,
    pub wtlb: *const TlbEntry,
    /// The paging of the active context (the TLBs hold translations of it alone).
    pub paging: Option<Long64ProbeConfig>,
    /// The page privilege of the accesses that filled the TLBs of the active context: `Ring3`
    /// for the context of CPL3, `Ring0` otherwise (ADR-0023 decision 7, slice C5a).
    pub privilege: PrivilegeLevel,
}

/// The key of a translation context: the walker snapshot (CR0, CR3, CR4, EFER), RFLAGS.AC and the
/// CPL (ADR-0023 decision 7: "the key of the translation slot includes the CPL"). The rights of a
/// cached translation are those of the accesses of its context: a context of CPL3 holds only
/// translations filled by user accesses, one of CPL0 only by supervisor ones, so neither
/// generated code nor `view_host` checks U/S or R/W on a hit. The CPL and AC also keep the code of
/// different code modes apart: the lookup table of a context is consulted before `code_for`, and
/// the code mode is a function of this key (plan C5, contract 4.9).
#[derive(Clone, Copy, PartialEq, Eq)]
struct SlotKey {
    paging: Long64ProbeConfig,
    ac: bool,
    cpl: u8,
}

/// One cached translation context (TLBs and the lookup table of one [`SlotKey`]).
struct Slot {
    key: Option<SlotKey>,
    rtlb: Box<[TlbEntry]>,
    wtlb: Box<[TlbEntry]>,
    lookup: Box<[LookupEntry]>,
    lookup_page: Box<[u64]>,
    lookup_leaf: Box<[u64]>,
    stamp: u64,
}

impl Slot {
    fn new() -> Self {
        Self {
            key: None,
            rtlb: vec![EMPTY_TLB; TLB_SIZE].into_boxed_slice(),
            wtlb: vec![EMPTY_TLB; TLB_SIZE].into_boxed_slice(),
            lookup: vec![EMPTY_LOOKUP; LOOKUP_SIZE].into_boxed_slice(),
            lookup_page: vec![u64::MAX; LOOKUP_SIZE].into_boxed_slice(),
            lookup_leaf: vec![u64::MAX; LOOKUP_SIZE].into_boxed_slice(),
            stamp: 0,
        }
    }
    fn flush(&mut self) {
        self.rtlb.fill(EMPTY_TLB);
        self.wtlb.fill(EMPTY_TLB);
        self.lookup.fill(EMPTY_LOOKUP);
    }
    fn drop_leaf(&mut self, frame: u64) {
        for entry in self.rtlb.iter_mut().chain(self.wtlb.iter_mut()) {
            if entry.leaf == frame {
                *entry = EMPTY_TLB;
            }
        }
        for (entry, leaf) in self.lookup.iter_mut().zip(self.lookup_leaf.iter()) {
            if *leaf == frame {
                *entry = EMPTY_LOOKUP;
            }
        }
    }
}

/// An entry filled through a leaf table frame (the arrays never move).
#[derive(Clone, Copy)]
enum LeafRef {
    Tlb(*mut TlbEntry),
    Lookup(*mut LookupEntry, *mut u64),
}

/// The shortest list of the leaf reverse index that is compacted.
const MIN_COMPACTION: usize = 256;

/// The mark of a reference seen once during a compaction (`pad` of a TLB entry, a high bit of a
/// lookup entry's leaf; frames are far below it). Set and cleared within one compaction.
const SEEN: u64 = 1 << 62;

impl LeafRef {
    fn holds(self, frame: u64) -> bool {
        // SAFETY: pointers into the engine's TLB and lookup arrays, alive while the engine is.
        unsafe {
            match self {
                LeafRef::Tlb(entry) => (*entry).leaf == frame,
                LeafRef::Lookup(_, leaf) => *leaf & !SEEN == frame,
            }
        }
    }

    fn clear(self) {
        // SAFETY: as above.
        unsafe {
            match self {
                LeafRef::Tlb(entry) => *entry = EMPTY_TLB,
                LeafRef::Lookup(entry, _) => *entry = EMPTY_LOOKUP,
            }
        }
    }

    /// Mark the entry seen; whether it was marked already (a second reference to it).
    fn mark(self) -> bool {
        // SAFETY: as above.
        unsafe {
            match self {
                LeafRef::Tlb(entry) => std::mem::replace(&mut (*entry).pad, SEEN) == SEEN,
                LeafRef::Lookup(_, leaf) => {
                    let seen = *leaf & SEEN != 0;
                    *leaf |= SEEN;
                    seen
                }
            }
        }
    }

    fn unmark(self) {
        // SAFETY: as above.
        unsafe {
            match self {
                LeafRef::Tlb(entry) => (*entry).pad = 0,
                LeafRef::Lookup(_, leaf) => *leaf &= !SEEN,
            }
        }
    }
}

/// Keep the references of `refs` whose entry still holds `frame`, each entry once (a slot refilled
/// through the same leaf is noted again on every fill); one pass, no sort.
fn compact_refs(refs: &mut Vec<LeafRef>, frame: u64) {
    refs.retain(|reference| reference.holds(frame) && !reference.mark());
    for reference in refs.iter() {
        reference.unmark();
    }
}

/// Cached translation contexts besides the active one.
const SLOTS: usize = 3;

/// The RIP of an empty lookup entry: non-canonical, so no guest RIP ever matches it (generated
/// code checks every RIP it takes from guest data for canonicity, and the general step never
/// leaves a non-canonical RIP).
const EMPTY_LOOKUP_RIP: u64 = 1 << 63;
const EMPTY_LOOKUP: LookupEntry = LookupEntry {
    rip: EMPTY_LOOKUP_RIP,
    code: 0,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LookupEntry {
    pub rip: u64,
    pub code: u64,
}

/// What generated code reads and writes through x21.
#[repr(C)]
pub(crate) struct TurboCtx {
    pub budget: i64,               // 0
    pub nzcv: u64,                 // 8
    pub pf: u64,                   // 16
    pub af: u64,                   // 24
    pub defined: u64,              // 32
    pub rtlb: *mut TlbEntry,       // 40
    pub wtlb: *mut TlbEntry,       // 48
    pub lookup: *mut LookupEntry,  // 56
    pub mem_helper: usize,         // 64
    pub env: *mut RunEnv<'static>, // 72
    pub status: u64,               // 80
    pub rep_helper: usize,         // 88
    /// The visible ES, CS, SS, DS, FS, GS selectors (bit 16: modelled), copied at the start of a run.
    pub selectors: [u32; 6], // 96
    pub rdtsc_helper: usize,       // 120
    /// The budget at the start of the current segment (after the last RDTSC transition).
    pub seg_start: i64,
    /// Instructions retired by the run in the segments before the current one.
    pub run_retired: u64,
    /// The interrupt source has a vector held back by IF = 0 (every boundary counts a refusal).
    pub masked: u64,
    pub div_helper: usize, // 152
    pub sti_helper: usize, // 160
    /// S7-32: the XMM registers of the FP member (`GuestFpState::sse.xmm`) when the SSE templates
    /// may run (the member is there and its #UD/#NM gate is open), 0 otherwise; set by each run.
    pub xmm: usize, // 168
    pub cmps_helper: usize, // 176
    /// S7-32: the operands of the last compare of a REPE CMPS (`cmps_helper`).
    pub cmp_a: u64, // 184
    pub cmp_b: u64,        // 192
}

pub(crate) const CTX_BUDGET: u32 = 0;
pub(crate) const CTX_NZCV: u32 = 8;
pub(crate) const CTX_PF: u32 = 16;
pub(crate) const CTX_AF: u32 = 24;
pub(crate) const CTX_DEFINED: u32 = 32;
pub(crate) const CTX_RTLB: u32 = 40;
pub(crate) const CTX_WTLB: u32 = 48;
pub(crate) const CTX_LOOKUP: u32 = 56;
pub(crate) const CTX_MEM_HELPER: u32 = 64;
pub(crate) const CTX_REP_HELPER: u32 = 88;
pub(crate) const CTX_SELECTORS: u32 = 96;
pub(crate) const CTX_RDTSC_HELPER: u32 = 120;
pub(crate) const CTX_SEG_START: u32 = 128;
pub(crate) const CTX_DIV_HELPER: u32 = 152;
pub(crate) const CTX_STI_HELPER: u32 = 160;
pub(crate) const CTX_XMM: u32 = 168;
pub(crate) const CTX_CMPS_HELPER: u32 = 176;
pub(crate) const CTX_CMP_A: u32 = 184;
pub(crate) const CTX_CMP_B: u32 = 192;
// S7-32: the fields generated code reaches by these offsets are where the offsets say.
const _: () = assert!(std::mem::offset_of!(TurboCtx, xmm) == CTX_XMM as usize);
const _: () = assert!(std::mem::offset_of!(TurboCtx, cmps_helper) == CTX_CMPS_HELPER as usize);
const _: () = assert!(std::mem::offset_of!(TurboCtx, cmp_a) == CTX_CMP_A as usize);
const _: () = assert!(std::mem::offset_of!(TurboCtx, cmp_b) == CTX_CMP_B as usize);

/// The code mode of a run: which generated code the engine may run (ADR-0023 decision 7). It is a
/// key of the code cache (`blocks`, `singles`, `crossings`), computed once per run by the set loop
/// from the state; the engine has no field "current mode" (no global CPL state). A function of the
/// translation context's [`SlotKey`] (CPL and AC there, CR0.AM in the walker snapshot): the lookup
/// table of a context holds code of one mode only. Slice C5b adds `User` (templates at CPL3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CodeMode {
    /// CPL0: every template of `jit::plan`.
    Supervisor,
    /// CPL3 without templates (slice C5a): the engine decodes, keeps the code pages and the
    /// tables watched and hands the general step its decode; it runs no generated instruction.
    UserNoCode,
}

impl CodeMode {
    /// Every mode (the code of a written page is dropped for each).
    pub(crate) const ALL: [CodeMode; 2] = [CodeMode::Supervisor, CodeMode::UserNoCode];

    /// The mode of a run from `state` under `set`: CPL0 `Supervisor`; CPL3 `UserNoCode` only when
    /// the CPL3 entry admits the state with the set (`od_isa::execute::cpl3_entry_admits`: CS.L = 1,
    /// DPL 3, the caches, RPL SS 3, the cells of the set), otherwise `None`: the entry refuses before
    /// any fetch, so the engine must not walk, decode or watch either (review F1); CPL 1 and 2 `None`
    /// (not reachable: the CPL0 entry refuses RPL 1 and 2). With `None` the engine does not run.
    pub(crate) fn of(
        state: &LongModeState,
        set: &od_isa::execute::Cpl0ProfileSet<'_>,
    ) -> Option<CodeMode> {
        match cpl_of(state) {
            0 => Some(CodeMode::Supervisor),
            3 if od_isa::execute::cpl3_entry_admits(state, set) => Some(CodeMode::UserNoCode),
            _ => None,
        }
    }
}

/// What a run needs from the set loop (borrowed for the duration of one run).
pub(crate) struct RunEnv<'a> {
    pub ram: &'a PhysicalMemory,
    pub paging: Long64ProbeConfig,
    pub context: AccessContext,
    pub engine: *mut TurboEngine,
    /// The RDTSC source of the set loop.
    pub rdtsc: Option<*mut (dyn FnMut() -> u64 + 'static)>,
    /// The board's fast transition between RDTSC quanta.
    pub fast: Option<
        *mut (
            dyn FnMut(
                    FastQuantumEnd,
                    Option<&dyn crate::irq_source::BoardIrqSource>,
                ) -> Option<FastQuantumStart>
                + 'static
        ),
    >,
    /// The interrupt hook of the set loop.
    pub irq: Option<*mut crate::irq_source::IrqBoundaryHook<'static>>,
    /// The set loop's retired count when the run started.
    pub executed_at_start: u64,
    /// The set loop's `quantum_base` and `instruction_budget`.
    pub quantum_base: *mut u64,
    pub instruction_budget: *mut u64,
    pub cpl: usize,
    /// The code mode of the run ([`CodeMode::of`] of the state the run starts from).
    pub mode: CodeMode,
    pub tick: Option<*const std::sync::atomic::AtomicBool>,
    /// The TSC an RDTSC of the current (merged) quantum reads, when the fast transition knows it.
    pub tsc_next: Option<u64>,
    /// S7-32: the cell of the set's FP member (`Cpl0ProfileSet::fp_state`), not borrowed while the
    /// run lasts: the SSE templates run on its XMM registers.
    pub fp: Option<*mut od_isa::execute::GuestFpState>,
    /// S7-32: the MMIO dispatch of the set loop's step view (the windows and devices the general
    /// step reaches), for the `OP_MMIO` accesses.
    pub mmio: Option<*const (dyn crate::bus::SetStepMmio + 'static)>,
}

impl RunEnv<'_> {
    /// Whether the accesses of the run are user accesses (CPL3): the helpers of the engine make
    /// only explicit and stack accesses, whose page privilege is the privilege of the context
    /// (`AccessContext::page_privilege`); the implicit supervisor accesses to descriptor tables
    /// are general steps.
    pub(crate) fn user(&self) -> bool {
        self.context.privilege == PrivilegeLevel::Ring3
    }
}

#[repr(C)]
pub(crate) struct HelperResult {
    pub value: u64,
    pub status: u64,
}

/// The bytes of executable memory the owner gives the engine ([`TurboEngine::new`]): 96 MiB, well
/// inside the ±128 MiB reach of the direct branches from blocks to the shared stubs.
pub const TURBO_ARENA_BYTES: usize = 96 << 20;

/// The largest arena the engine takes: every direct branch inside it must reach the stubs at its
/// start (`b`/`bl` reach ±128 MiB).
const MAX_ARENA_BYTES: usize = 128 << 20;

/// The executable region of the engine: the owner's [`ExecutableArena`] (the process's MAP_JIT
/// mapping, owned by `od-host`), allocated in order by the engine itself.
struct CodeArena {
    exec: Box<dyn ExecutableArena>,
    base: usize,
    size: usize,
    used: usize,
}

impl CodeArena {
    fn new(exec: Box<dyn ExecutableArena>) -> Option<Self> {
        let (base, size) = (exec.base(), exec.len());
        if size > MAX_ARENA_BYTES || base % 16 != 0 {
            return None;
        }
        Some(Self {
            exec,
            base,
            size,
            used: 0,
        })
    }

    fn reserve(&self, words: usize) -> Option<usize> {
        let bytes = words * 4;
        if self.used + bytes > self.size {
            return None;
        }
        Some(self.base + self.used)
    }

    /// Copy `words` to `at` (from `reserve`) and advance.
    fn commit(&mut self, at: usize, words: &[u32]) {
        debug_assert_eq!(at, self.base + self.used);
        self.exec.write(at, words);
        self.used += words.len() * 4;
        // Keep the next block 16-byte aligned.
        self.used = (self.used + 15) & !15;
    }

    /// Overwrite one word in place.
    fn patch(&mut self, at: usize, word: u32) {
        self.exec.write(at, &[word]);
    }

    fn reset(&mut self, keep: usize) {
        self.used = keep;
    }
}

/// Test only: an executable region for the tests of this crate (production regions come from the
/// owner, `od_host::JitArena`). `None` off macOS/aarch64.
#[cfg(test)]
pub(crate) fn test_arena() -> Option<Box<dyn ExecutableArena>> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        use std::ffi::c_void;
        unsafe extern "C" {
            fn mmap(
                addr: *mut c_void,
                len: usize,
                prot: i32,
                flags: i32,
                fd: i32,
                off: i64,
            ) -> *mut c_void;
            fn pthread_jit_write_protect_np(enabled: i32);
            fn sys_icache_invalidate(start: *mut c_void, len: usize);
        }
        struct TestArena {
            base: usize,
            len: usize,
        }
        // SAFETY: a private mapping the test owns; it is never unmapped (tests are short-lived).
        unsafe impl Send for TestArena {}
        impl ExecutableArena for TestArena {
            fn base(&self) -> usize {
                self.base
            }
            fn len(&self) -> usize {
                self.len
            }
            fn write(&mut self, at: usize, words: &[u32]) {
                let bytes = words.len() * 4;
                assert!(at >= self.base && at + bytes <= self.base + self.len);
                // SAFETY: inside the test's MAP_JIT mapping; write protection is lifted for this
                // thread only while copying.
                unsafe {
                    pthread_jit_write_protect_np(0);
                    std::ptr::copy_nonoverlapping(
                        words.as_ptr().cast::<u8>(),
                        at as *mut u8,
                        bytes,
                    );
                    pthread_jit_write_protect_np(1);
                    sys_icache_invalidate(at as *mut c_void, bytes);
                }
            }
        }
        let len = 16 << 20;
        // SAFETY: an anonymous private MAP_JIT mapping (RWX, MAP_PRIVATE | MAP_ANON | MAP_JIT).
        let raw = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                7,
                0x0002 | 0x1000 | 0x0800,
                -1,
                0,
            )
        };
        if raw as isize == -1 || raw.is_null() {
            return None;
        }
        Some(Box::new(TestArena {
            base: raw as usize,
            len,
        }))
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        None
    }
}

/// Offsets inside `LongModeState`.
#[derive(Clone, Copy)]
pub(crate) struct StateOffsets {
    pub gpr: u32,
    pub rip: u32,
    pub rflags: u32,
    pub fs_base: u32,
    pub gs_base: u32,
}

#[derive(Clone, Copy)]
struct BlockInfo {
    code: usize,
    page: u64,
}

/// Outcome of one run of generated code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TurboExit {
    /// The budget is used up (or less than the next block needs).
    Budget,
    /// The instruction at `state.rip` is not translated: the general path runs it.
    Slow,
    /// `state.rip` is an RDTSC (bare `0F 31`): the loop executes it.
    Rdtsc,
    /// A REP string instruction at `state.rip` stopped at the iteration limit (not retired).
    RepPartial,
    /// S7-32: the device refused the access of the instruction at `state.rip` (not retired);
    /// [`TurboEngine::take_device_error`] gives the error the general step's access would have
    /// returned (the device was reached once).
    DeviceError,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TurboOutcome {
    pub retired: u64,
    pub exit: TurboExit,
}

/// The engine: code cache, lookup table, TLBs and watches.
pub(crate) struct TurboEngine {
    arena: CodeArena,
    entry: usize,
    exit_common: usize,
    lookup_stub: usize,
    slow_stub: usize,
    rdtsc_stub: usize,
    stubs_end: usize,
    ctx: Box<TurboCtx>,
    rtlb: Box<[TlbEntry]>,
    wtlb: Box<[TlbEntry]>,
    lookup: Box<[LookupEntry]>,
    /// The physical page of each lookup entry's code.
    lookup_page: Box<[u64]>,
    /// The leaf table frame of each lookup entry's code translation.
    lookup_leaf: Box<[u64]>,
    /// Inactive translation contexts.
    slots: Vec<Slot>,
    view: Box<TlbView>,
    stamp: u64,
    /// Blocks by (RIP, physical page, code mode).
    blocks: HashMap<(u64, u64, CodeMode), BlockInfo>,
    /// Physical page -> keys of the blocks on it.
    code_pages: HashMap<u64, Vec<(u64, u64, CodeMode)>>,
    /// Physical page -> bitmap of its bytes that compiled instructions came from.
    code_bytes: HashMap<u64, Box<[u64; 64]>>,
    /// Physical pages known to be page-table frames of cached translations: bit 0 a leaf
    /// table, bit 1 an upper level.
    pt_frames: HashMap<u64, u8>,
    offsets: StateOffsets,
    paging_key: Option<SlotKey>,
    singles: HashMap<(u64, u64, CodeMode), BlockInfo>,
    /// Blocks of one instruction crossing a page, by (RIP, first physical page): the physical
    /// page of the rest and the code. Kept out of `blocks`: every install walks the second page
    /// again.
    crossings: HashMap<(u64, u64, CodeMode), (u64, usize)>,
    /// Per physical page: the RIPs of its singles, slow decodes and stub lookups.
    page_extras: HashMap<u64, Vec<u64>>,
    /// Per leaf table frame: the index of the list of TLB and lookup entries filled through it.
    leaf_refs: HashMap<u64, u32>,
    ref_lists: Vec<Vec<LeafRef>>,
    /// Per list of `ref_lists`: the length at which it is compacted next.
    ref_compact_at: Vec<usize>,
    free_lists: Vec<u32>,
    /// The frame of the last noted reference and its list.
    last_ref: (u64, u32),
    /// Decodes of instructions the generated code stops before, by (RIP, physical page); without
    /// the code mode: the decoder does not depend on the CPL.
    slow_insns: HashMap<(u64, u64), od_isa::long_decode::Instruction>,
    /// Decodes made by the last translation (filled by `jit::compile_limited`).
    pub(crate) pending_decodes: Vec<(u64, od_isa::long_decode::Instruction)>,
    /// The decode of the instruction at which the last run stopped.
    slow_decode: Option<(u64, od_isa::long_decode::Instruction)>,
    /// The physical page of the code the last run was in when it stopped.
    last_page: u64,
    /// The leaf table frame of the last code translation (for `install`).
    last_leaf: u64,
    pub stats: TurboStats,
    /// S7-32: the profile of the slow exits (`enable_profile`; `None` otherwise).
    profile: Option<Box<SlowProfile>>,
    /// S7-32: why a helper last refused (read and cleared by the profile at a slow exit).
    refusal: Option<Refusal>,
    /// S7-32: the error of a device access of generated code (`OP_MMIO`), for the loop.
    device_error: Option<crate::page_walk::TrackedAccessError>,
}

// SAFETY: the raw pointers of the engine point at allocations it owns (its TLB and lookup arrays,
// its context, its code arena) or, only during `run`, at the run's environment; the engine is used
// by one thread at a time (the board that owns it), so moving it to another thread moves all of it.
unsafe impl Send for TurboEngine {}

#[derive(Clone, Copy, Debug, Default)]
pub struct TurboStats {
    pub runs: u64,
    pub retired: u64,
    pub compiled_blocks: u64,
    pub compiled_instructions: u64,
    pub slow_exits: u64,
    pub lookups: u64,
    pub tlb_fills: u64,
    pub tlb_fallbacks: u64,
    pub flushes: u64,
    pub code_invalidations: u64,
    pub general_steps: u64,
    pub code_writes: u64,
    pub leaf_drops: u64,
    pub slot_switches: u64,
    pub slot_misses: u64,
    pub singles: u64,
    pub rep_calls: u64,
    pub rep_pages: u64,
    pub rdtsc_calls: u64,
    pub fast_transitions: u64,
    pub helper_reads: u64,
    pub helper_writes: u64,
    /// S7-32: accesses of generated code that went to a device (`OP_MMIO`).
    pub mmio_accesses: u64,
    /// S7-32: reads of generated code served from ROM (as the general step's `rom_read`).
    pub rom_reads: u64,
    /// S7-32: REPE CMPS executions of generated code (`cmps_helper`).
    pub cmps_calls: u64,
}

impl TurboEngine {
    /// An engine generating code into `arena`; `None` on a host whose code the engine does not
    /// emit (it emits AArch64 only) or for an arena it cannot use.
    pub(crate) fn new(arena: Box<dyn ExecutableArena>) -> Option<Box<Self>> {
        if !cfg!(target_arch = "aarch64") {
            return None;
        }
        let offsets = StateOffsets {
            gpr: std::mem::offset_of!(LongModeState, gpr) as u32,
            rip: std::mem::offset_of!(LongModeState, rip) as u32,
            rflags: std::mem::offset_of!(LongModeState, rflags) as u32,
            fs_base: std::mem::offset_of!(LongModeState, fs_base) as u32,
            gs_base: std::mem::offset_of!(LongModeState, gs_base) as u32,
        };
        // Generated code reaches the fields with scaled unsigned immediates: bytes of any GPR (the
        // high-byte registers too) below 4096, 8-byte fields 8-aligned below 32768.
        let qword = |offset: u32| offset % 8 == 0 && offset <= 32760;
        if offsets.gpr + 16 * 8 > 4095
            || ![
                offsets.gpr,
                offsets.rip,
                offsets.rflags,
                offsets.fs_base,
                offsets.gs_base,
            ]
            .into_iter()
            .all(qword)
        {
            return None;
        }
        let arena = CodeArena::new(arena)?;
        let mut rtlb = vec![EMPTY_TLB; TLB_SIZE].into_boxed_slice();
        let mut wtlb = vec![EMPTY_TLB; TLB_SIZE].into_boxed_slice();
        let mut lookup = vec![EMPTY_LOOKUP; LOOKUP_SIZE].into_boxed_slice();
        let ctx = Box::new(TurboCtx {
            budget: 0,
            nzcv: 0,
            pf: 0,
            af: 0,
            defined: 0,
            rtlb: rtlb.as_mut_ptr(),
            wtlb: wtlb.as_mut_ptr(),
            lookup: lookup.as_mut_ptr(),
            mem_helper: mem_helper as *const () as usize,
            env: std::ptr::null_mut(),
            status: 0,
            rep_helper: rep_helper as *const () as usize,
            selectors: [0; 6],
            rdtsc_helper: rdtsc_helper as *const () as usize,
            seg_start: 0,
            run_retired: 0,
            masked: 0,
            div_helper: div_helper as *const () as usize,
            sti_helper: sti_helper as *const () as usize,
            xmm: 0,
            cmps_helper: cmps_helper as *const () as usize,
            cmp_a: 0,
            cmp_b: 0,
        });
        let mut engine = Box::new(Self {
            arena,
            entry: 0,
            exit_common: 0,
            lookup_stub: 0,
            slow_stub: 0,
            rdtsc_stub: 0,
            stubs_end: 0,
            ctx,
            rtlb,
            wtlb,
            lookup,
            lookup_page: vec![u64::MAX; LOOKUP_SIZE].into_boxed_slice(),
            lookup_leaf: vec![u64::MAX; LOOKUP_SIZE].into_boxed_slice(),
            slots: (0..SLOTS).map(|_| Slot::new()).collect(),
            view: Box::new(TlbView {
                rtlb: std::ptr::null(),
                wtlb: std::ptr::null(),
                paging: None,
                privilege: PrivilegeLevel::Ring0,
            }),
            stamp: 0,
            blocks: HashMap::default(),
            code_pages: HashMap::default(),
            code_bytes: HashMap::default(),
            pt_frames: HashMap::default(),
            offsets,
            paging_key: None,
            singles: HashMap::default(),
            crossings: HashMap::default(),
            page_extras: HashMap::default(),
            leaf_refs: HashMap::default(),
            ref_lists: Vec::new(),
            ref_compact_at: Vec::new(),
            free_lists: Vec::new(),
            last_ref: (u64::MAX, 0),
            slow_insns: HashMap::default(),
            slow_decode: None,
            pending_decodes: Vec::new(),
            last_page: u64::MAX,
            last_leaf: u64::MAX,
            stats: TurboStats::default(),
            profile: None,
            refusal: None,
            device_error: None,
        });
        engine.emit_stubs();
        Some(engine)
    }

    fn emit_stubs(&mut self) {
        use od_jit::arm64::a64::*;
        // entry(state x0, ctx x1, code x2)
        let mut asm = Asm::new();
        asm.emit(stp_pre(29, 30, SP, -96));
        asm.emit(stp(19, 20, SP, 16));
        asm.emit(stp(21, 22, SP, 32));
        asm.emit(stp(23, 24, SP, 48));
        asm.emit(stp(25, 26, SP, 64));
        asm.emit(stp(27, 28, SP, 80));
        asm.emit(mov_sp(29, SP));
        asm.emit(mov_reg(true, 19, 0));
        asm.emit(mov_reg(true, 21, 1));
        asm.emit(ldr_imm(8, 20, 21, CTX_BUDGET));
        asm.emit(ldr_imm(8, 22, 21, CTX_RTLB));
        asm.emit(ldr_imm(8, 23, 21, CTX_WTLB));
        asm.emit(ldr_imm(8, 28, 21, CTX_LOOKUP));
        asm.emit(ldr_imm(8, 24, 21, CTX_DEFINED));
        asm.emit(ldp(26, 27, 21, CTX_PF as i32));
        asm.emit(ldr_imm(8, 9, 21, CTX_NZCV));
        asm.emit(msr_nzcv(9));
        asm.emit(br(2));
        let entry_words = asm.words.len();
        // exit_common (x0 reason, x1 rip)
        let exit_at = entry_words;
        asm.emit(str_imm(8, 1, 19, self.offsets.rip));
        asm.emit(str_imm(8, 20, 21, CTX_BUDGET));
        asm.emit(str_imm(8, 24, 21, CTX_DEFINED));
        asm.emit(stp(26, 27, 21, CTX_PF as i32));
        asm.emit(mrs_nzcv(9));
        asm.emit(str_imm(8, 9, 21, CTX_NZCV));
        asm.emit(ldp(27, 28, SP, 80));
        asm.emit(ldp(25, 26, SP, 64));
        asm.emit(ldp(23, 24, SP, 48));
        asm.emit(ldp(21, 22, SP, 32));
        asm.emit(ldp(19, 20, SP, 16));
        asm.emit(ldp_post(29, 30, SP, 96));
        asm.emit(ret());
        // lookup (x0 rip)
        let lookup_at = asm.words.len();
        let miss = asm.label();
        asm.emit(eor_reg_shift(true, 9, 0, 0, LSR, 12));
        asm.emit(and_imm(true, 9, 9, (LOOKUP_SIZE - 1) as u64).unwrap());
        asm.emit(add_reg(true, 9, 28, 9, LSL, 4));
        asm.emit(ldp(10, 11, 9, 0));
        asm.emit(eor_reg(true, 10, 10, 0));
        asm.cbnz_to(true, 10, miss);
        asm.emit(br(11));
        asm.bind(miss);
        asm.emit(mov_reg(true, 1, 0));
        asm.emit(movz(true, 0, EXIT_LOOKUP as u32, 0));
        let exit_rel = exit_at as i32 - asm.words.len() as i32;
        asm.emit(b(exit_rel));
        // slow stub: the lookup table entry of an untranslatable RIP (x0 rip)
        let slow_at = asm.words.len();
        asm.emit(mov_reg(true, 1, 0));
        asm.emit(movz(true, 0, EXIT_SLOW as u32, 0));
        let exit_rel = exit_at as i32 - asm.words.len() as i32;
        asm.emit(b(exit_rel));
        // rdtsc stub: entry of an RDTSC (x0 rip)
        let rdtsc_at = asm.words.len();
        asm.emit(mov_reg(true, 1, 0));
        asm.emit(movz(true, 0, EXIT_RDTSC as u32, 0));
        let exit_rel = exit_at as i32 - asm.words.len() as i32;
        asm.emit(b(exit_rel));
        let base = self.arena.reserve(asm.words.len()).expect("stubs fit");
        asm.finish(base).expect("stubs resolve");
        self.arena.commit(base, &asm.words);
        self.entry = base;
        self.exit_common = base + exit_at * 4;
        self.lookup_stub = base + lookup_at * 4;
        self.slow_stub = base + slow_at * 4;
        self.rdtsc_stub = base + rdtsc_at * 4;
        self.stubs_end = self.arena.used;
    }

    fn flush_write_tlb(&mut self) {
        self.wtlb.fill(EMPTY_TLB);
        for slot in &mut self.slots {
            slot.wtlb.fill(EMPTY_TLB);
        }
    }

    fn flush_lookup(&mut self) {
        self.lookup.fill(EMPTY_LOOKUP);
        for slot in &mut self.slots {
            slot.lookup.fill(EMPTY_LOOKUP);
        }
    }

    /// Every translation the engine cached (TLBs, lookup tables, page-table watch), in every
    /// context.
    pub(crate) fn flush_translations(&mut self, ram: &PhysicalMemory) {
        self.stats.flushes += 1;
        for &frame in self.pt_frames.keys() {
            ram.set_table_watch(GuestPhysicalAddress::new(frame << 12), false);
        }
        self.rtlb.fill(EMPTY_TLB);
        self.wtlb.fill(EMPTY_TLB);
        self.lookup.fill(EMPTY_LOOKUP);
        for slot in &mut self.slots {
            slot.flush();
        }
        self.pt_frames.clear();
        self.leaf_refs.clear();
        self.ref_lists.clear();
        self.ref_compact_at.clear();
        self.free_lists.clear();
        self.last_ref = (u64::MAX, 0);
    }

    /// A table frame was written: the translations through it are dropped (only those whose
    /// leaf it holds when it is a leaf table only; everything otherwise).
    fn table_written(&mut self, ram: &PhysicalMemory, frame: u64) {
        match self.pt_frames.get(&frame).copied() {
            None => {}
            Some(levels) if levels & 2 != 0 => self.flush_translations(ram),
            Some(_) => {
                self.stats.leaf_drops += 1;
                for reference in self.take_leaf_refs(frame) {
                    if reference.holds(frame) {
                        reference.clear();
                    }
                }
                self.pt_frames.remove(&frame);
                ram.set_table_watch(GuestPhysicalAddress::new(frame << 12), false);
            }
        }
    }

    fn note_leaf_ref(&mut self, frame: u64, reference: LeafRef) {
        if frame == u64::MAX {
            return;
        }
        let index = if self.last_ref.0 == frame {
            self.last_ref.1
        } else {
            let index = match self.leaf_refs.get(&frame) {
                Some(&index) => index,
                None => {
                    let index = match self.free_lists.pop() {
                        Some(index) => index,
                        None => {
                            self.ref_lists.push(Vec::new());
                            self.ref_compact_at.push(0);
                            (self.ref_lists.len() - 1) as u32
                        }
                    };
                    self.ref_compact_at[index as usize] = MIN_COMPACTION;
                    self.leaf_refs.insert(frame, index);
                    index
                }
            };
            self.last_ref = (frame, index);
            index
        };
        let refs = &mut self.ref_lists[index as usize];
        let compact_at = &mut self.ref_compact_at[index as usize];
        if refs.len() >= *compact_at {
            // Compact: the entries that still hold the frame, each once (a slot refilled through
            // the same leaf is noted again on every fill). The list then stays within the number
            // of TLB and lookup entries of all contexts; the next compaction waits for as many
            // new references as survived (amortized constant work per reference).
            compact_refs(refs, frame);
            *compact_at = MIN_COMPACTION.max(2 * refs.len());
        }
        refs.push(reference);
    }

    /// The entries filled through leaf table `frame` (the list is emptied).
    fn take_leaf_refs(&mut self, frame: u64) -> Vec<LeafRef> {
        let Some(index) = self.leaf_refs.remove(&frame) else {
            return Vec::new();
        };
        if self.last_ref.0 == frame {
            self.last_ref = (u64::MAX, 0);
        }
        self.free_lists.push(index);
        std::mem::take(&mut self.ref_lists[index as usize])
    }

    /// Note the table frames of a walk (PML4 first; the last one holds the leaf) and watch the new
    /// ones (every committed write to them is logged by `PhysicalMemory`); `true` when one of them
    /// was not watched before. Returns the leaf frame too (`u64::MAX` without tables).
    fn watch_tables(
        &mut self,
        ram: &PhysicalMemory,
        tables: &[Option<GuestPhysicalAddress>; 4],
    ) -> (bool, u64) {
        let count = tables.iter().flatten().count();
        let mut new = false;
        let mut leaf = u64::MAX;
        for (index, table) in tables.iter().flatten().enumerate() {
            let frame = table.get() >> 12;
            let bit = if index + 1 == count { 1 } else { 2 };
            let levels = self.pt_frames.entry(frame).or_insert(0);
            if *levels == 0 {
                new = true;
                ram.set_table_watch(GuestPhysicalAddress::new(frame << 12), true);
            }
            *levels |= bit;
            leaf = frame;
        }
        (new, leaf)
    }

    /// Keep the decodes of the last translation (their bytes count as code of the page).
    fn keep_decodes(&mut self, page: u64) {
        let decodes = std::mem::take(&mut self.pending_decodes);
        let mut ranges = Vec::with_capacity(decodes.len());
        for (rip, instruction) in &decodes {
            if (rip & 0xfff) + u64::from(instruction.length) <= 0x1000 {
                if self.slow_insns.insert((*rip, page), *instruction).is_none() {
                    self.page_extras.entry(page).or_default().push(*rip);
                }
                ranges.push(((rip & 0xfff) as u16, instruction.length));
            }
        }
        self.mark_code(page, &ranges);
        self.code_pages.entry(page).or_default();
        self.pending_decodes = decodes;
        self.pending_decodes.clear();
    }

    fn mark_code(&mut self, page: u64, ranges: &[(u16, u8)]) {
        let map = self
            .code_bytes
            .entry(page)
            .or_insert_with(|| Box::new([0; 64]));
        for &(offset, length) in ranges {
            for byte in offset as usize..(offset as usize + length as usize).min(4096) {
                map[byte / 64] |= 1 << (byte % 64);
            }
        }
    }

    /// Whether the bytes `[offset, offset + len)` of `page` (clipped to the page) hold code.
    fn overlaps_code(&self, page: u64, offset: u64, len: u64) -> bool {
        let Some(map) = self.code_bytes.get(&page) else {
            return false;
        };
        let end = (offset + len).min(4096);
        let mut byte = offset;
        while byte < end {
            let word = (byte / 64) as usize;
            let bit = byte % 64;
            let take = (64 - bit).min(end - byte);
            let mask = if take == 64 {
                u64::MAX
            } else {
                ((1u64 << take) - 1) << bit
            };
            if map[word] & mask != 0 {
                return true;
            }
            byte += take;
        }
        false
    }

    /// A write of `[address, address + len)` (guest physical): drop the blocks it changes.
    fn note_code_write(&mut self, ram: &PhysicalMemory, address: u64, len: u64) {
        self.stats.code_writes += 1;
        if len == 0 {
            return;
        }
        let first = address >> 12;
        let last = (address + len - 1) >> 12;
        for page in first..=last {
            if !self.code_bytes.contains_key(&page) {
                continue;
            }
            let start = if page == first { address & 0xfff } else { 0 };
            let end = if page == last {
                ((address + len - 1) & 0xfff) + 1
            } else {
                4096
            };
            if self.overlaps_code(page, start, end - start) {
                self.invalidate_code_page(ram, page);
            }
        }
    }

    /// Drop the blocks of physical page `page` (its bytes changed).
    fn invalidate_code_page(&mut self, ram: &PhysicalMemory, page: u64) {
        self.code_bytes.remove(&page);
        if let Some(keys) = self.code_pages.remove(&page) {
            self.stats.code_invalidations += 1;
            for key in keys {
                self.blocks.remove(&key);
                self.crossings.remove(&key);
                self.drop_lookup_of(key.0, key.1);
            }
        }
        if let Some(rips) = self.page_extras.remove(&page) {
            for rip in rips {
                for mode in CodeMode::ALL {
                    self.singles.remove(&(rip, page, mode));
                }
                self.slow_insns.remove(&(rip, page));
                self.drop_lookup_of(rip, page);
            }
        }
        ram.set_code_watch(GuestPhysicalAddress::new(page << 12), false);
    }

    /// Writes `PhysicalMemory` logged to watched pages since the last call (by the general step,
    /// devices, DMA, any run without the engine): drop the translations through written table
    /// frames and the blocks of written code.
    pub(crate) fn drain_code_watch(&mut self, ram: &PhysicalMemory) {
        if let Some((ranges, overflowed)) = ram.take_code_watch_hits() {
            if overflowed {
                // More writes than the log keeps: what they changed is unknown, start over.
                self.drop_code(ram);
                self.flush_translations(ram);
                return;
            }
            for (address, len) in ranges {
                if len == 0 {
                    continue;
                }
                let first = address >> 12;
                let last = (address + len - 1) >> 12;
                for page in first..=last {
                    if self.pt_frames.contains_key(&page) {
                        self.table_written(ram, page);
                    }
                }
                self.note_code_write(ram, address, len);
            }
        }
    }

    fn check_paging(&mut self, state: &LongModeState, paging: &Long64ProbeConfig) {
        let key = SlotKey {
            paging: *paging,
            ac: state.rflags & (1 << 18) != 0,
            cpl: cpl_of(state),
        };
        if self.paging_key == Some(key) {
            return;
        }
        self.stamp += 1;
        let index = match self.slots.iter().position(|slot| slot.key == Some(key)) {
            Some(index) => index,
            None => {
                // The least recently used context gives way.
                let index = (0..SLOTS)
                    .min_by_key(|&index| self.slots[index].stamp)
                    .unwrap_or(0);
                self.slots[index].flush();
                self.slots[index].key = Some(key);
                self.stats.slot_misses += 1;
                index
            }
        };
        let slot = &mut self.slots[index];
        std::mem::swap(&mut self.rtlb, &mut slot.rtlb);
        std::mem::swap(&mut self.wtlb, &mut slot.wtlb);
        std::mem::swap(&mut self.lookup, &mut slot.lookup);
        std::mem::swap(&mut self.lookup_page, &mut slot.lookup_page);
        std::mem::swap(&mut self.lookup_leaf, &mut slot.lookup_leaf);
        let previous = self.paging_key;
        self.paging_key = slot.key;
        slot.key = previous;
        slot.stamp = self.stamp;
        self.ctx.rtlb = self.rtlb.as_mut_ptr();
        self.ctx.wtlb = self.wtlb.as_mut_ptr();
        self.ctx.lookup = self.lookup.as_mut_ptr();
        self.stats.slot_switches += 1;
        self.sync_view();
    }

    fn install(&mut self, rip: u64, code: usize, page: u64) {
        let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
        self.lookup[index] = LookupEntry {
            rip,
            code: code as u64,
        };
        self.lookup_page[index] = page;
        self.lookup_leaf[index] = self.last_leaf;
        let reference = LeafRef::Lookup(&mut self.lookup[index], &mut self.lookup_leaf[index]);
        let leaf = self.last_leaf;
        self.note_leaf_ref(leaf, reference);
        if code == self.rdtsc_stub {
            self.page_extras.entry(page).or_default().push(rip);
        }
    }

    /// Every translated block, single and remembered decode (the code cache starts empty).
    fn drop_code(&mut self, ram: &PhysicalMemory) {
        self.arena.reset(self.stubs_end);
        self.blocks.clear();
        self.crossings.clear();
        self.singles.clear();
        self.code_bytes.clear();
        self.slow_insns.clear();
        self.page_extras.clear();
        for page in self.code_pages.keys() {
            ram.set_code_watch(GuestPhysicalAddress::new(page << 12), false);
        }
        self.code_pages.clear();
        self.slow_decode = None;
        self.flush_lookup();
    }

    /// Forget everything the engine derived from guest memory and paging: guest memory changed
    /// without passing through the engine (a platform reset, a restored snapshot).
    pub(crate) fn reset(&mut self, ram: &PhysicalMemory) {
        // Writes logged before the reset concern code that is dropped below.
        let _ = ram.take_code_watch_hits();
        self.drop_code(ram);
        self.flush_translations(ram);
        for slot in &mut self.slots {
            slot.key = None;
        }
        self.paging_key = None;
        self.last_page = u64::MAX;
        self.last_leaf = u64::MAX;
        self.device_error = None;
        self.refusal = None;
        self.sync_view();
    }

    /// Drop the lookup entries (of every context) of the block `(rip, page)`.
    fn drop_lookup_of(&mut self, rip: u64, page: u64) {
        let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
        if self.lookup[index].rip == rip && self.lookup_page[index] == page {
            self.lookup[index] = EMPTY_LOOKUP;
        }
        for slot in &mut self.slots {
            if slot.lookup[index].rip == rip && slot.lookup_page[index] == page {
                slot.lookup[index] = EMPTY_LOOKUP;
            }
        }
    }

    /// The code for `rip`: an existing block, a fresh one, or the slow/RDTSC stub. `None` when
    /// the fetch itself needs the general path (a fault, not RAM).
    fn code_for(&mut self, env: &mut RunEnv<'_>, rip: u64) -> Option<usize> {
        self.stats.lookups += 1;
        if let Some(walk) = fast_translate(env.ram, &env.paging, rip, WalkKind::Fetch, env.user()) {
            apply_ad(&walk, false);
            return self.code_for_physical(
                env,
                rip,
                GuestPhysicalAddress::new(walk.physical),
                walk.tables,
                walk.host_page,
            );
        }
        // The fetch walk of the general path (accessed bits) for the first byte of the page.
        let request = TrackedAccessRequest {
            instruction_rip: rip,
            address: GuestLinearAddress::new(rip),
            context: env.context,
            operation: RamAccessOperation::Fetch,
        };
        let mut wrote = false;
        let report = crate::page_walk::access_long64_2m_ram(env.ram, &env.paging, request, |_| {
            wrote = true;
        });
        report.outcome.ok()?;
        let (physical, tables) = crate::page_walk::data_translation_of_walk_trace(
            &report.trace,
            GuestLinearAddress::new(rip),
        )?;
        env.ram
            .ram_frame(GuestPhysicalAddress::new(physical.get() & !0xfff))?;
        let host_page = env.ram.ram_only_page_host_ptr(physical)?;
        self.code_for_physical(env, rip, physical, tables, host_page)
    }

    fn code_for_physical(
        &mut self,
        env: &mut RunEnv<'_>,
        rip: u64,
        physical: GuestPhysicalAddress,
        tables: [Option<GuestPhysicalAddress>; 4],
        host_page: *mut u8,
    ) -> Option<usize> {
        let page = physical.get() >> 12;
        let (new, leaf) = self.watch_tables(env.ram, &tables);
        if new {
            self.flush_write_tlb();
        }
        self.last_leaf = leaf;
        let mode = env.mode;
        if let Some(info) = self.blocks.get(&(rip, page, mode)) {
            let code = info.code;
            self.install(rip, code, page);
            return Some(code);
        }
        self.pending_decodes.clear();
        let compiled = jit::compile(self, rip, page, host_page as *const u8, env.context, mode);
        self.keep_decodes(page);
        if !matches!(compiled, jit::Compiled::NoSpace) && !env.ram.code_watch(physical) {
            env.ram.set_code_watch(physical, true);
            // No write entry may point at a page with code.
            self.flush_write_tlb();
        }
        let code = match compiled {
            jit::Compiled::Block(code, instructions, ranges) => {
                self.mark_code(page, &ranges);
                self.stats.compiled_blocks += 1;
                self.stats.compiled_instructions += instructions as u64;
                self.blocks
                    .insert((rip, page, mode), BlockInfo { code, page });
                self.code_pages
                    .entry(page)
                    .or_default()
                    .push((rip, page, mode));
                if !env.ram.code_watch(physical) {
                    env.ram.set_code_watch(physical, true);
                    // No write entry may point at a code page.
                    self.flush_write_tlb();
                }
                code
            }
            jit::Compiled::Slow => {
                // Remembered like a block, so that a flush of the lookup table does not
                // translate it again.
                self.blocks.insert(
                    (rip, page, mode),
                    BlockInfo {
                        code: self.slow_stub,
                        page,
                    },
                );
                self.code_pages
                    .entry(page)
                    .or_default()
                    .push((rip, page, mode));
                self.slow_stub
            }
            jit::Compiled::Rdtsc => self.rdtsc_stub,
            jit::Compiled::Crossing => match self.crossing_block(env, rip, physical) {
                Some(code) => code,
                None => self.slow_stub,
            },
            jit::Compiled::NoSpace => {
                // Start over with an empty code cache.
                self.drop_code(env.ram);
                return None;
            }
        };
        self.install(rip, code, page);
        Some(code)
    }

    /// A block of the one instruction at `rip` (for a budget smaller than the block there).
    fn single_for(&mut self, env: &mut RunEnv<'_>, rip: u64) -> Option<usize> {
        self.stats.singles += 1;
        let request = TrackedAccessRequest {
            instruction_rip: rip,
            address: GuestLinearAddress::new(rip),
            context: env.context,
            operation: RamAccessOperation::Fetch,
        };
        let report = crate::page_walk::access_long64_2m_ram(env.ram, &env.paging, request, |_| {});
        report.outcome.ok()?;
        let (physical, _) = crate::page_walk::data_translation_of_walk_trace(
            &report.trace,
            GuestLinearAddress::new(rip),
        )?;
        let page = physical.get() >> 12;
        let mode = env.mode;
        if let Some(info) = self.singles.get(&(rip, page, mode)) {
            return Some(info.code);
        }
        let host_page = env.ram.ram_page_host_ptr(physical)?;
        self.pending_decodes.clear();
        let compiled = jit::compile_limited(
            self,
            rip,
            page,
            host_page as *const u8,
            env.context,
            mode,
            1,
            None,
        );
        self.keep_decodes(page);
        if !env.ram.code_watch(physical) {
            env.ram.set_code_watch(physical, true);
            self.flush_write_tlb();
        }
        match compiled {
            jit::Compiled::Block(code, _, ranges) => {
                self.mark_code(page, &ranges);
                self.singles
                    .insert((rip, page, mode), BlockInfo { code, page });
                self.page_extras.entry(page).or_default().push(rip);
                self.code_pages.entry(page).or_default();
                if !env.ram.code_watch(physical) {
                    env.ram.set_code_watch(physical, true);
                    self.flush_write_tlb();
                }
                Some(code)
            }
            jit::Compiled::Slow => None,
            jit::Compiled::NoSpace => {
                // Start over with an empty code cache (the next lookup translates again).
                self.drop_code(env.ram);
                None
            }
            jit::Compiled::Crossing => self.crossing_block(env, rip, physical),
            jit::Compiled::Rdtsc => Some(self.rdtsc_stub_for(rip)),
        }
    }

    /// A block of the one instruction at `rip` whose bytes continue on the next page.
    fn crossing_block(
        &mut self,
        env: &mut RunEnv<'_>,
        rip: u64,
        physical: GuestPhysicalAddress,
    ) -> Option<usize> {
        let next_linear = (rip | 0xfff).wrapping_add(1);
        // The fetch walk of the next page (its accessed bits), as the general path's fetch.
        let request = TrackedAccessRequest {
            instruction_rip: rip,
            address: GuestLinearAddress::new(next_linear),
            context: env.context,
            operation: RamAccessOperation::Fetch,
        };
        let report = crate::page_walk::access_long64_2m_ram(env.ram, &env.paging, request, |_| {});
        report.outcome.ok()?;
        let (next_physical, tables) = crate::page_walk::data_translation_of_walk_trace(
            &report.trace,
            GuestLinearAddress::new(next_linear),
        )?;
        let (new, _) = self.watch_tables(env.ram, &tables);
        if new {
            self.flush_write_tlb();
        }
        // A crossing block depends on two translations; its lookup entry names the leaf of the
        // first, so any write to the second's leaf table drops all translations (as a write to an
        // upper level does).
        if let Some(frame) = tables.iter().flatten().last() {
            *self.pt_frames.entry(frame.get() >> 12).or_insert(0) |= 2;
        }
        let page = physical.get() >> 12;
        let next_page = next_physical.get() >> 12;
        let mode = env.mode;
        // A block translated before for the same two physical pages (the walk above is made on
        // every install, so a changed second translation is never missed).
        if let Some(&(known_next, code)) = self.crossings.get(&(rip, page, mode))
            && known_next == next_page
        {
            return Some(code);
        }
        let host = env.ram.ram_page_host_ptr(physical)?;
        let next_host = env.ram.ram_page_host_ptr(next_physical)?;
        self.pending_decodes.clear();
        let compiled = jit::compile_limited(
            self,
            rip,
            page,
            host as *const u8,
            env.context,
            mode,
            1,
            Some(next_host as *const u8),
        );
        self.pending_decodes.clear();
        let code = match compiled {
            jit::Compiled::Block(code, _, _) => code,
            jit::Compiled::NoSpace => {
                // Start over with an empty code cache (the next lookup translates again).
                self.drop_code(env.ram);
                return None;
            }
            _ => return None,
        };
        // Both pages hold its bytes: the tail of the first, the head of the second.
        let offset = (rip & 0xfff) as u16;
        self.mark_code(page, &[(offset, (0x1000 - u32::from(offset)) as u8)]);
        self.mark_code(next_page, &[(0, 16)]);
        for (p, phys) in [(page, physical), (next_page, next_physical)] {
            let keys = self.code_pages.entry(p).or_default();
            if !keys.contains(&(rip, page, mode)) {
                keys.push((rip, page, mode));
            }
            if !env.ram.code_watch(phys) {
                env.ram.set_code_watch(phys, true);
            }
        }
        self.flush_write_tlb();
        self.crossings.insert((rip, page, mode), (next_page, code));
        self.stats.compiled_blocks += 1;
        Some(code)
    }

    fn rdtsc_stub_for(&self, _rip: u64) -> usize {
        self.rdtsc_stub
    }

    /// The engine's TLBs for the general path's adapter.
    pub(crate) fn tlbs(&mut self) -> *const TlbView {
        self.sync_view();
        &*self.view
    }

    fn sync_view(&mut self) {
        self.view.rtlb = self.rtlb.as_ptr();
        self.view.wtlb = self.wtlb.as_ptr();
        self.view.paging = self.paging_key.map(|key| key.paging);
        // Without a key the view has no paging and answers nothing.
        self.view.privilege = match self.paging_key {
            Some(key) if key.cpl == 3 => PrivilegeLevel::Ring3,
            _ => PrivilegeLevel::Ring0,
        };
    }

    /// The decode of the instruction at `rip` the last run ended before, if the engine has it.
    pub(crate) fn take_slow_decode(
        &mut self,
        rip: u64,
    ) -> Option<(u64, od_isa::long_decode::Instruction)> {
        match self.slow_decode.take() {
            Some((at, instruction)) if at == rip => Some((at, instruction)),
            _ => None,
        }
    }

    /// The loop runs one general step at `state.rip` (an instruction the engine leaves to it).
    pub(crate) fn note_general_step(&mut self, state: &LongModeState) {
        self.stats.general_steps += 1;
        if let Some(profile) = self.profile.as_deref_mut() {
            profile.general_step(state.rip, cpl_of(state));
        }
    }

    /// S7-32: the error of the device access the last run ended with ([`TurboExit::DeviceError`]).
    pub(crate) fn take_device_error(&mut self) -> Option<crate::page_walk::TrackedAccessError> {
        self.device_error.take()
    }

    /// S7-32: profile the slow exits from now on (reported by [`Self::report`]).
    pub(crate) fn enable_profile(&mut self) {
        if self.profile.is_none() {
            self.profile = Some(Box::new(SlowProfile::new()));
        }
    }

    /// The statistics of the engine as text, then the profile of the slow exits when it is on.
    pub(crate) fn report(&self) -> String {
        let mut text = format!("{:?}", self.stats);
        if let Some(profile) = self.profile.as_deref() {
            text.push('\n');
            text.push_str(&profile.report(40));
        }
        text
    }

    /// The memory helper refuses the access at linear `address` for `refusal` (the general step
    /// runs the instruction).
    fn refuse(&mut self, refusal: Refusal, address: u64) {
        self.refusal = Some(refusal);
        if let Some(profile) = self.profile.as_deref_mut() {
            profile.mem_refusal(refusal, address);
        }
    }

    /// The reason and the decode of the slow exit at `rip` the last run ended with (profile only).
    fn slow_site(
        &mut self,
        env: &RunEnv<'_>,
        rip: u64,
        no_code: bool,
    ) -> (SlowReason, Option<od_isa::long_decode::Instruction>) {
        let refusal = self.refusal.take();
        let instruction = match self.slow_decode {
            Some((_, instruction)) => Some(instruction),
            None => crate::paged_memory_adapter::histogram_decode(
                rip,
                env.ram,
                &env.paging,
                env.context,
            ),
        };
        let reason = match instruction {
            _ if no_code => SlowReason::NoCode,
            None => SlowReason::NoCode,
            // A CPL3 form without generated code is classified as at CPL0: whether a template
            // exists for it (slice C5a; the profile line carries the CPL and the operation).
            Some(instruction)
                if !jit::admitted(
                    &instruction,
                    match env.mode {
                        CodeMode::UserNoCode => CodeMode::Supervisor,
                        mode => mode,
                    },
                ) =>
            {
                SlowReason::NoTemplate
            }
            Some(_) => match refusal {
                Some(refusal) => SlowReason::Refused(refusal),
                None => {
                    let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
                    let entry = self.lookup[index];
                    if entry.rip == rip && entry.code as usize == self.slow_stub {
                        // Admitted, yet the RIP has the slow stub: a page-crossing instruction
                        // without a block.
                        SlowReason::NoCode
                    } else {
                        SlowReason::Refused(Refusal::Check)
                    }
                }
            },
        };
        (reason, instruction)
    }

    /// Run generated code from `state.rip` for at most `budget` instructions.
    pub(crate) fn run(
        &mut self,
        env: &mut RunEnv<'_>,
        state: &mut LongModeState,
        rflags_defined: &mut u64,
        budget: u64,
        masked: bool,
    ) -> TurboOutcome {
        self.stats.runs += 1;
        if let Some(profile) = self.profile.as_deref_mut() {
            profile.enter();
        }
        self.drain_code_watch(env.ram);
        self.check_paging(state, &env.paging);
        env.engine = self as *mut _;
        // Guest flags into the host form.
        let rflags = state.rflags;
        self.ctx.nzcv = ((rflags & SF != 0) as u64) << 31
            | ((rflags & ZF != 0) as u64) << 30
            | ((rflags & CF == 0) as u64) << 29
            | ((rflags & OF != 0) as u64) << 28;
        self.ctx.pf = if rflags & PF != 0 { 0 } else { 1 };
        self.ctx.af = if rflags & AF != 0 { 0x10 } else { 0 };
        self.ctx.defined = *rflags_defined & ARITH;
        self.ctx.budget = budget.min(i64::MAX as u64) as i64;
        self.ctx.env = (env as *mut RunEnv<'_>).cast();
        let known = |selector: Option<u16>| selector.map_or(0, |value| u32::from(value) | 1 << 16);
        self.ctx.selectors = [
            known(Some(state.es_selector)),
            known(state.cs_selector),
            known(state.ss_selector),
            known(Some(state.ds_selector)),
            known(Some(state.fs_selector)),
            known(Some(state.gs_selector)),
        ];
        self.ctx.seg_start = self.ctx.budget;
        self.ctx.run_retired = 0;
        self.ctx.masked = masked as u64;
        // S7-32: the SSE templates run where the general step would run the form: the FP member
        // is there and its gate is open (CR0.EM = 0, CR4.OSFXSR = 1, CR0.TS = 0; the control
        // registers do not change inside a run: their writes are general steps).
        self.ctx.xmm = match env.fp {
            // SAFETY: the cell of the set's FP member, alive and not borrowed for the run.
            Some(fp) if sse_gate_open(state) => unsafe { (*fp).sse.xmm.as_mut_ptr() as usize },
            _ => 0,
        };
        let mut exit;
        // The engine had no code for the RIP (profile only).
        let mut no_code = false;
        loop {
            if self.ctx.budget <= 0 {
                exit = TurboExit::Budget;
                break;
            }
            let rip = state.rip;
            let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
            let entry = self.lookup[index];
            let code = if entry.rip == rip {
                entry.code as usize
            } else {
                match self.code_for(env, rip) {
                    Some(code) => code,
                    None => {
                        no_code = true;
                        exit = TurboExit::Slow;
                        break;
                    }
                }
            };
            if code == self.slow_stub {
                exit = TurboExit::Slow;
                break;
            }
            if code == self.rdtsc_stub {
                exit = TurboExit::Rdtsc;
                break;
            }
            type Entry = unsafe extern "C" fn(*mut LongModeState, *mut TurboCtx, usize) -> u64;
            // SAFETY: `entry` is the trampoline emitted by `emit_stubs`; it saves the callee-saved
            // registers, runs generated code that touches only `state`, the context and guest RAM
            // through the TLB, and returns through `exit_common`.
            let reason = unsafe {
                let f: Entry = std::mem::transmute(self.entry);
                f(state, &mut *self.ctx, code)
            };
            match reason {
                EXIT_LOOKUP => continue,
                EXIT_BUDGET => {
                    if self.ctx.budget <= 0 {
                        exit = TurboExit::Budget;
                        break;
                    }
                    // Less budget than the block: one instruction at a time.
                    match self.single_for(env, state.rip) {
                        Some(code) if code == self.rdtsc_stub => {
                            exit = TurboExit::Rdtsc;
                            break;
                        }
                        Some(code) => {
                            // SAFETY: as above.
                            let reason = unsafe {
                                let f: Entry = std::mem::transmute(self.entry);
                                f(state, &mut *self.ctx, code)
                            };
                            match reason {
                                EXIT_LOOKUP | EXIT_BUDGET => continue,
                                EXIT_SLOW => {
                                    exit = TurboExit::Slow;
                                    break;
                                }
                                EXIT_RDTSC => {
                                    exit = TurboExit::Rdtsc;
                                    break;
                                }
                                EXIT_REP_PARTIAL => {
                                    exit = TurboExit::RepPartial;
                                    break;
                                }
                                EXIT_BOUNDARY => {
                                    exit = TurboExit::Budget;
                                    break;
                                }
                                other => panic!("turbo: unknown exit {other}"),
                            }
                        }
                        None => {
                            no_code = true;
                            exit = TurboExit::Slow;
                            break;
                        }
                    }
                }
                EXIT_SLOW => {
                    exit = TurboExit::Slow;
                    break;
                }
                EXIT_RDTSC => {
                    exit = TurboExit::Rdtsc;
                    break;
                }
                EXIT_REP_PARTIAL => {
                    exit = TurboExit::RepPartial;
                    break;
                }
                EXIT_BOUNDARY => {
                    exit = TurboExit::Budget;
                    break;
                }
                other => panic!("turbo: unknown exit {other}"),
            }
        }
        self.drain_code_watch(env.ram);
        // Host flags back into RFLAGS.
        let nzcv = self.ctx.nzcv;
        let mut flags = state.rflags & !ARITH;
        if nzcv & (1 << 31) != 0 {
            flags |= SF;
        }
        if nzcv & (1 << 30) != 0 {
            flags |= ZF;
        }
        if nzcv & (1 << 29) == 0 {
            flags |= CF;
        }
        if nzcv & (1 << 28) != 0 {
            flags |= OF;
        }
        if (self.ctx.pf as u8).count_ones() % 2 == 0 {
            flags |= PF;
        }
        if self.ctx.af & 0x10 != 0 {
            flags |= AF;
        }
        state.rflags = flags;
        *rflags_defined = (*rflags_defined & !ARITH) | (self.ctx.defined & ARITH);
        let last = (self.ctx.seg_start - self.ctx.budget.max(0)) as u64;
        let retired = self.ctx.run_retired + last;
        // Every boundary between two instructions of the last segment polled the source.
        if self.ctx.masked != 0 && last > 1 {
            if let Some(irq) = env.irq {
                // SAFETY: the hook of the set loop, alive for the run.
                unsafe { (*irq).counters.irq_refused_masked += last - 1 };
            }
        }
        self.ctx.env = std::ptr::null_mut();
        if exit == TurboExit::Slow && self.device_error.is_some() {
            // The device refused the access: the call ends as the general step's would.
            exit = TurboExit::DeviceError;
        }
        if exit == TurboExit::Slow {
            self.stats.slow_exits += 1;
            let rip = state.rip;
            let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
            let known_page = (self.lookup[index].rip == rip).then(|| self.lookup_page[index]);
            self.slow_decode = if let Some(page) = known_page {
                self.slow_insns
                    .get(&(rip, page))
                    .copied()
                    .map(|instruction| (rip, instruction))
            } else {
                crate::page_walk::translate_long64_2m_ram_read_only(
                    env.ram,
                    &env.paging,
                    rip,
                    GuestLinearAddress::new(rip),
                    env.context,
                    AccessKind::Fetch,
                )
                .ok()
                .and_then(|physical| self.slow_insns.get(&(rip, physical.get() >> 12)).copied())
                .map(|instruction| (rip, instruction))
            };
        }
        if self.profile.is_some() {
            let slow = (exit == TurboExit::Slow).then(|| {
                let (reason, instruction) = self.slow_site(env, state.rip, no_code);
                (state.rip, cpl_of(state), reason, instruction)
            });
            if let Some(profile) = self.profile.as_deref_mut() {
                profile.leave(exit, slow);
            }
        }
        self.stats.retired += retired;
        TurboOutcome { retired, exit }
    }

    /// Fill a TLB entry from a fast walk of `address`.
    fn fill_fast(&mut self, ram: &PhysicalMemory, address: u64, walk: &FastWalk, write: bool) {
        let page = walk.physical >> 12;
        let (new_tables, leaf) = self.watch_tables(ram, &walk.tables);
        if new_tables {
            self.flush_write_tlb();
        }
        if write && (self.pt_frames.contains_key(&page) || self.code_bytes.contains_key(&page)) {
            return;
        }
        let linear_page = address >> 12;
        let index = (linear_page as usize) & (TLB_SIZE - 1);
        let entry = TlbEntry {
            tag: linear_page,
            addend: (walk.host_page as u64).wrapping_sub(linear_page << 12),
            leaf,
            pad: 0,
        };
        self.stats.tlb_fills += 1;
        let reference = if write {
            self.wtlb[index] = entry;
            LeafRef::Tlb(&mut self.wtlb[index])
        } else {
            self.rtlb[index] = entry;
            LeafRef::Tlb(&mut self.rtlb[index])
        };
        self.note_leaf_ref(entry.leaf, reference);
    }

    /// Fill a TLB entry from a successful walk.
    fn fill(
        &mut self,
        ram: &PhysicalMemory,
        address: u64,
        trace: &crate::page_walk::WalkTrace,
        write: bool,
    ) {
        let Some((physical, tables)) = crate::page_walk::data_translation_of_walk_trace(
            trace,
            GuestLinearAddress::new(address),
        ) else {
            return;
        };
        let Some(host) = ram.ram_page_host_ptr(physical) else {
            return;
        };
        for table in tables.iter().flatten() {
            if !matches!(
                ram.locate_region_kind(*table, 0x1000),
                Ok(crate::MemoryRegionKind::Ram)
            ) {
                return;
            }
        }
        let page = physical.get() >> 12;
        let (new_tables, leaf) = self.watch_tables(ram, &tables);
        if new_tables {
            self.flush_write_tlb();
        }
        if write
            && (self.pt_frames.contains_key(&page)
                || ram.code_watch(physical)
                || self.code_bytes.contains_key(&page))
        {
            return;
        }
        let linear_page = address >> 12;
        let index = (linear_page as usize) & (TLB_SIZE - 1);
        let entry = TlbEntry {
            tag: linear_page,
            addend: (host as u64).wrapping_sub(linear_page << 12),
            leaf,
            pad: 0,
        };
        self.stats.tlb_fills += 1;
        let reference = if write {
            self.wtlb[index] = entry;
            LeafRef::Tlb(&mut self.wtlb[index])
        } else {
            self.rtlb[index] = entry;
            LeafRef::Tlb(&mut self.rtlb[index])
        };
        self.note_leaf_ref(entry.leaf, reference);
    }
}

fn width_of(size: u64) -> crate::AccessWidth {
    match size {
        1 => crate::AccessWidth::Byte,
        2 => crate::AccessWidth::Word,
        4 => crate::AccessWidth::Dword,
        _ => crate::AccessWidth::Qword,
    }
}

/// The slow path of a guest data access from generated code: one access with the semantics of the
/// general path (walk with A/D, RAM only), and the TLB filled from its walk. Status 0: done (the
/// value of a read is returned); 1: the general path must run the instruction (nothing happened
/// besides accessed/dirty bits the general path sets again).
unsafe extern "C" fn mem_helper(
    ctx: *mut TurboCtx,
    address: u64,
    op: u64,
    value: u64,
    rip: u64,
) -> HelperResult {
    // SAFETY: called only from generated code during `TurboEngine::run`, which set `env`.
    let ctx = unsafe { &mut *ctx };
    let env = unsafe { &mut *ctx.env };
    let engine = unsafe { &mut *env.engine };
    let size = op & 0xf;
    let write = op & OP_WRITE != 0;
    if write {
        engine.stats.helper_writes += 1;
    } else {
        engine.stats.helper_reads += 1;
    }
    let fallback = HelperResult {
        value: 0,
        status: 1,
    };
    if op & OP_FILL128 != 0 {
        return fill128(engine, env, address, write, rip);
    }
    if (address & 0xfff) + size > 0x1000 {
        engine.stats.tlb_fallbacks += 1;
        engine.refuse(Refusal::CrossPage, address);
        return fallback;
    }
    let width = width_of(size);
    // The fast form of the walk: read-only checks, then the accessed/dirty bits and the access.
    let kind = if write {
        WalkKind::Write
    } else {
        WalkKind::Read
    };
    if let Some(walk) = fast_translate(env.ram, &env.paging, address, kind, env.user()) {
        let page = walk.physical >> 12;
        // SAFETY: the host page of the walk holds the whole access (no page crossing).
        let host = unsafe { walk.host_page.add((address & 0xfff) as usize) };
        if op & OP_ATOMIC != 0 {
            if engine.pt_frames.contains_key(&page)
                || engine.code_bytes.contains_key(&page)
                || walk
                    .tables
                    .iter()
                    .flatten()
                    .any(|table| table.get() >> 12 == page)
            {
                engine.stats.tlb_fallbacks += 1;
                engine.refuse(Refusal::Watched, address);
                return fallback;
            }
            apply_ad(&walk, true);
            engine.fill_fast(env.ram, address, &walk, true);
            engine.fill_fast(env.ram, address, &walk, false);
            let linear_page = address >> 12;
            if engine.wtlb[(linear_page as usize) & (TLB_SIZE - 1)].tag == linear_page {
                return HelperResult {
                    value: 0,
                    status: 0,
                };
            }
            engine.refuse(Refusal::Watched, address);
            return fallback;
        }
        if write {
            if engine.pt_frames.contains_key(&page)
                || engine.overlaps_code(page, walk.physical & 0xfff, size)
                || walk
                    .tables
                    .iter()
                    .flatten()
                    .any(|table| table.get() >> 12 == page)
            {
                engine.stats.tlb_fallbacks += 1;
                engine.refuse(Refusal::Watched, address);
                return fallback;
            }
            apply_ad(&walk, true);
            // SAFETY: as above.
            unsafe {
                match size {
                    1 => *host = value as u8,
                    2 => host.cast::<u16>().write_unaligned(value as u16),
                    4 => host.cast::<u32>().write_unaligned(value as u32),
                    _ => host.cast::<u64>().write_unaligned(value),
                }
            }
            engine.fill_fast(env.ram, address, &walk, true);
            return HelperResult {
                value: 0,
                status: 0,
            };
        }
        apply_ad(&walk, false);
        // SAFETY: as above.
        let value = unsafe {
            match size {
                1 => u64::from(*host),
                2 => u64::from(host.cast::<u16>().read_unaligned()),
                4 => u64::from(host.cast::<u32>().read_unaligned()),
                _ => host.cast::<u64>().read_unaligned(),
            }
        };
        engine.fill_fast(env.ram, address, &walk, false);
        return HelperResult { value, status: 0 };
    }
    if write {
        // Classify first, without effects: a watched page goes to the general path.
        let (physical, tables) =
            match crate::page_walk::translate_long64_2m_ram_read_only_with_tables(
                env.ram,
                &env.paging,
                rip,
                GuestLinearAddress::new(address),
                env.context,
                AccessKind::Write,
            ) {
                Ok(translation) => translation,
                Err(error) => {
                    engine.stats.tlb_fallbacks += 1;
                    engine.refuse(Refusal::of(&error), address);
                    return fallback;
                }
            };
        let page = physical.get() >> 12;
        if env.ram.ram_page_host_ptr(physical).is_none() {
            if op & OP_MMIO != 0
                && let Some(done) = mmio_access(engine, env, address, size, true, value, rip)
            {
                return done;
            }
            engine.stats.tlb_fallbacks += 1;
            engine.refuse(Refusal::NotRam, address);
            return fallback;
        }
        if engine.pt_frames.contains_key(&page)
            || engine.overlaps_code(page, physical.get() & 0xfff, size)
            || tables
                .iter()
                .flatten()
                .any(|table| table.get() >> 12 == page)
        {
            engine.stats.tlb_fallbacks += 1;
            engine.refuse(Refusal::Watched, address);
            return fallback;
        }
    }
    if op & OP_ATOMIC != 0 {
        let request = TrackedAccessRequest {
            instruction_rip: rip,
            address: GuestLinearAddress::new(address),
            context: env.context,
            operation: RamAccessOperation::AtomicTarget { width },
        };
        let report = crate::page_walk::access_long64_2m_ram(env.ram, &env.paging, request, |_| {});
        return match report.outcome {
            Ok(crate::page_walk::TrackedAccessOutcome::Located(_)) => {
                engine.fill(env.ram, address, &report.trace, true);
                engine.fill(env.ram, address, &report.trace, false);
                // Filled: run the instruction again (status 0); not filled: the general path.
                let page = address >> 12;
                let index = (page as usize) & (TLB_SIZE - 1);
                if engine.wtlb[index].tag == page {
                    HelperResult {
                        value: 0,
                        status: 0,
                    }
                } else {
                    engine.refuse(Refusal::Watched, address);
                    fallback
                }
            }
            Err(error) => {
                engine.refuse(Refusal::of(&error), address);
                fallback
            }
            _ => {
                engine.refuse(Refusal::Walk, address);
                fallback
            }
        };
    }
    let request = TrackedAccessRequest {
        instruction_rip: rip,
        address: GuestLinearAddress::new(address),
        context: env.context,
        operation: if write {
            RamAccessOperation::Write { width, value }
        } else {
            RamAccessOperation::Read { width }
        },
    };
    let report = crate::page_walk::access_long64_2m_ram(env.ram, &env.paging, request, |_| {});
    match report.outcome {
        Ok(crate::page_walk::TrackedAccessOutcome::Read(value)) => {
            engine.fill(env.ram, address, &report.trace, false);
            HelperResult { value, status: 0 }
        }
        Ok(crate::page_walk::TrackedAccessOutcome::Written) => {
            engine.fill(env.ram, address, &report.trace, true);
            HelperResult {
                value: 0,
                status: 0,
            }
        }
        Err(error) => {
            let refusal = Refusal::of(&error);
            // S7-32: a read of ROM, as the general step's adapter serves it (`rom_read`): the
            // bytes of the region, nothing else happens (the walk set what it sets for any read).
            if !write
                && let crate::page_walk::TrackedAccessError::OperandPhysical {
                    address: physical,
                    source: crate::MemoryAccessError::UnreadableRegion(crate::MemoryRegionKind::Rom),
                } = &error
            {
                let mut bytes = [0u8; 8];
                if env
                    .ram
                    .load_bytes(*physical, &mut bytes[..size as usize])
                    .is_ok()
                {
                    engine.stats.rom_reads += 1;
                    return HelperResult {
                        value: u64::from_le_bytes(bytes),
                        status: 0,
                    };
                }
            }
            if refusal == Refusal::NotRam
                && op & OP_MMIO != 0
                && !write
                && let Some(done) = mmio_access(engine, env, address, size, false, 0, rip)
            {
                return done;
            }
            engine.stats.tlb_fallbacks += 1;
            engine.refuse(refusal, address);
            fallback
        }
        _ => {
            engine.stats.tlb_fallbacks += 1;
            engine.refuse(Refusal::Walk, address);
            fallback
        }
    }
}

/// S7-32: an `OP_MMIO` access of `size` bytes at `address` that is not RAM, made as the general
/// step makes it: the tracked walk (accessed/dirty bits) and the dispatch of the set loop's step
/// view (its windows, its devices, its `mmio_written` and device-access count). `Some` with status
/// 2 (and the value of a read) when it was done; `Some` with status 1 and the error kept in the
/// engine when the dispatch failed (`MmioBackend`: the run ends with [`TurboExit::DeviceError`]
/// and the loop ends the call as the general step's failed access would, without reaching the
/// device again); `None` when the general path must run the instruction (no dispatch here, an
/// address no window serves, a fault: no device was reached and the walk changed nothing the
/// general step does not set again).
fn mmio_access(
    engine: &mut TurboEngine,
    env: &mut RunEnv<'_>,
    address: u64,
    size: u64,
    write: bool,
    value: u64,
    rip: u64,
) -> Option<HelperResult> {
    // SAFETY: the step view of the set loop, alive and not otherwise used during the run.
    let mmio = unsafe { &*env.mmio? };
    let width = width_of(size);
    let request = TrackedAccessRequest {
        instruction_rip: rip,
        address: GuestLinearAddress::new(address),
        context: env.context,
        operation: if write {
            RamAccessOperation::Write { width, value }
        } else {
            RamAccessOperation::Read { width }
        },
    };
    let report = crate::page_walk::access_long64_2m_with_mmio(
        env.ram,
        &env.paging,
        request,
        |_| {},
        mmio.windows(),
        |physical, width, operation| mmio.dispatch(physical, width, operation),
    );
    let value = match report.outcome {
        Ok(crate::page_walk::TrackedAccessOutcome::Read(value)) => value,
        Ok(crate::page_walk::TrackedAccessOutcome::Written) => 0,
        Err(error @ crate::page_walk::TrackedAccessError::MmioBackend { .. }) => {
            engine.device_error = Some(error);
            return Some(HelperResult {
                value: 0,
                status: 1,
            });
        }
        _ => return None,
    };
    engine.stats.mmio_accesses += 1;
    Some(HelperResult { value, status: 2 })
}

/// S7-32: the TLB entry for a 16-byte access at `address` of the SSE templates (`OP_FILL128`):
/// the fast walk with the accessed (and, for a write, dirty) bits the general path's walk sets,
/// then the fill. Status 0: the entry is there (the template checks again); 1: the general path
/// runs the instruction (a crossing, a walk the fast form does not take, a watched page: nothing
/// happened).
fn fill128(
    engine: &mut TurboEngine,
    env: &mut RunEnv<'_>,
    address: u64,
    write: bool,
    rip: u64,
) -> HelperResult {
    let fallback = HelperResult {
        value: 0,
        status: 1,
    };
    if (address & 0xfff) + 16 > 0x1000 {
        engine.stats.tlb_fallbacks += 1;
        engine.refuse(Refusal::CrossPage, address);
        return fallback;
    }
    let kind = if write {
        WalkKind::Write
    } else {
        WalkKind::Read
    };
    let Some(walk) = fast_translate(env.ram, &env.paging, address, kind, env.user()) else {
        engine.stats.tlb_fallbacks += 1;
        let refusal = if engine.profile.is_some() {
            // Why, for the profile only: the read-only walk of the general path.
            match crate::page_walk::translate_long64_2m_ram_read_only_with_tables(
                env.ram,
                &env.paging,
                rip,
                GuestLinearAddress::new(address),
                env.context,
                if write {
                    AccessKind::Write
                } else {
                    AccessKind::Read
                },
            ) {
                Err(error) => Refusal::of(&error),
                Ok((physical, _)) if env.ram.ram_page_host_ptr(physical).is_none() => {
                    Refusal::NotRam
                }
                Ok(_) => Refusal::Walk,
            }
        } else {
            Refusal::Walk
        };
        engine.refuse(refusal, address);
        return fallback;
    };
    let page = walk.physical >> 12;
    if write
        && (engine.pt_frames.contains_key(&page)
            || engine.code_bytes.contains_key(&page)
            || walk
                .tables
                .iter()
                .flatten()
                .any(|table| table.get() >> 12 == page))
    {
        engine.stats.tlb_fallbacks += 1;
        engine.refuse(Refusal::Watched, address);
        return fallback;
    }
    apply_ad(&walk, write);
    engine.fill_fast(env.ram, address, &walk, write);
    let linear_page = address >> 12;
    let tlb = if write { &engine.wtlb } else { &engine.rtlb };
    if tlb[(linear_page as usize) & (TLB_SIZE - 1)].tag == linear_page {
        HelperResult {
            value: 0,
            status: 0,
        }
    } else {
        engine.stats.tlb_fallbacks += 1;
        engine.refuse(Refusal::Watched, address);
        fallback
    }
}

/// The end of a logical quantum the set loop offers the board for a fast transition: the
/// instructions it retired and the CPL of the state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FastQuantumEnd {
    pub retired: u64,
    pub cpl: usize,
}

/// A new logical quantum the board granted: its budget and whether the deadline cut it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FastQuantumStart {
    pub budget: u64,
    pub cut_by_deadline: bool,
    /// The source was asked again (it caught up with the clock): its saved answer is stale.
    pub fresh_source: bool,
    /// What an RDTSC of the new quantum reads (the clock does not move inside a quantum).
    pub tsc: u64,
}

/// What the board hands the set loop in turbo mode.
pub(crate) struct TurboHook<'a> {
    pub engine: &'a mut TurboEngine,
    /// The RDTSC tick of the board (raised by every RDTSC).
    pub tick: Option<&'a std::sync::atomic::AtomicBool>,
    /// The board's transition between two quanta ended by an RDTSC tick alone, done without
    /// leaving the loop: `None` when the board must see the boundary itself.
    pub fast_quantum: Option<
        &'a mut dyn FnMut(
            FastQuantumEnd,
            Option<&dyn crate::irq_source::BoardIrqSource>,
        ) -> Option<FastQuantumStart>,
    >,
}

/// Whether the `#UD`/`#NM` gate of the SSE forms of the FP member is open: CR0.EM = 0 and
/// CR4.OSFXSR = 1 (else `#UD`), CR0.TS = 0 (else `#NM`), both registers carried (the set's
/// `sse_cpl0_gate`, `step_xmm_handler_form` and the S2-34/S2-38/S2-39 executors).
fn sse_gate_open(state: &LongModeState) -> bool {
    const CR0_EM: u64 = 1 << 2;
    const CR0_TS: u64 = 1 << 3;
    const CR4_OSFXSR: u64 = 1 << 9;
    matches!(
        (state.cr0, state.cr4),
        (Some(cr0), Some(cr4)) if cr0 & (CR0_EM | CR0_TS) == 0 && cr4 & CR4_OSFXSR != 0
    )
}

/// The CPL of `state` (the RPL of CS; 0 while CS is not modelled).
fn cpl_of(state: &LongModeState) -> u8 {
    state.cs_selector.map_or(0, |selector| (selector & 3) as u8)
}

/// Admission of generated code: no TF/RF, no STI/MOV SS shadow, running (the CPL decides
/// [`CodeMode::of`] in the set loop).
pub(crate) fn admits(state: &LongModeState) -> bool {
    const TF: u64 = 1 << 8;
    const RF: u64 = 1 << 16;
    state.rflags & (TF | RF) == 0
        && state.sti_shadow == Some(od_isa::StiShadow::Clear)
        && state
            .activity
            .is_none_or(|activity| activity == od_isa::CpuActivity::Running)
}

/// REP MOVS/STOS of generated code (forward, 64-bit addresses): the iterations of the general path
/// (`REP_ITERATION_WORK_LIMIT` at most per execution) when every page they touch is plain RAM and
/// no written page is watched. A read-only check of every page first, in the order of the first
/// touch; then the iterations, the first access of every page through the tracked walk (accessed
/// and dirty bits as the general path sets them), the others through the host page. Status 0: done
/// and retired; 1: nothing happened, the general path runs it; 2: the limit was reached (registers
/// moved, not retired).
unsafe extern "C" fn rep_helper(
    ctx: *mut TurboCtx,
    state: *mut LongModeState,
    params: u64,
    length: u64,
    rip: u64,
) -> u64 {
    // SAFETY: called only from generated code during `TurboEngine::run`, which set `env`; the
    // engine is reached through the raw pointer only before and after `rep_run`.
    let engine = unsafe { (*(*ctx).env).engine };
    let started = unsafe { (*engine).profile.is_some() }.then(std::time::Instant::now);
    // SAFETY: as for `rep_helper`.
    let status = unsafe { rep_run(ctx, state, params, length, rip) };
    // SAFETY: `rep_run` returned; nothing else borrows the engine.
    let engine = unsafe { &mut *engine };
    if status == 1 {
        engine.refusal = Some(Refusal::Rep);
    }
    // Only with the profile (`started`): no second test of the engine's profile without it.
    if let Some(started) = started
        && let Some(profile) = engine.profile.as_deref_mut()
    {
        profile.rep_nanos += started.elapsed().as_nanos() as u64;
        match status {
            1 => profile.rep_refused += 1,
            2 => profile.rep_partial += 1,
            _ => {}
        }
    }
    status
}

/// The body of [`rep_helper`].
unsafe fn rep_run(
    ctx: *mut TurboCtx,
    state: *mut LongModeState,
    params: u64,
    length: u64,
    rip: u64,
) -> u64 {
    // SAFETY: called only from generated code during `TurboEngine::run`.
    let ctx = unsafe { &mut *ctx };
    let env = unsafe { &mut *ctx.env };
    let engine = unsafe { &mut *env.engine };
    let state = unsafe { &mut *state };
    let size = params & 0xf;
    let movs = params & 0x10 != 0;
    engine.stats.rep_calls += 1;
    const DF: u64 = 1 << 10;
    if state.rflags & DF != 0 {
        return 1;
    }
    use od_isa::Register;
    let count = state.gpr[Register::Rcx as usize];
    if count == 0 {
        state.rip = rip.wrapping_add(length);
        return 0;
    }
    let limit = od_isa::execute::REP_ITERATION_WORK_LIMIT;
    let n = count.min(limit);
    let rsi = state.gpr[Register::Rsi as usize];
    let rdi = state.gpr[Register::Rdi as usize];
    // Every element inside one page (the general path splits a crossing element).
    if rdi & (size - 1) != 0 && (rdi & 0xfff) + n * size > 0x1000 {
        let crosses = (0..n).any(|i| {
            let a = rdi.wrapping_add(i * size);
            (a & 0xfff) + size > 0x1000
        });
        if crosses {
            return 1;
        }
    }
    if movs && rsi & (size - 1) != 0 && (rsi & 0xfff) + n * size > 0x1000 {
        let crosses = (0..n).any(|i| {
            let a = rsi.wrapping_add(i * size);
            (a & 0xfff) + size > 0x1000
        });
        if crosses {
            return 1;
        }
    }
    // Chunks: runs of elements with one source page and one destination page.
    struct Chunk {
        count: u64,
        src: usize,
        dst: usize,
    }
    let mut walks: Vec<(u64, FastWalk)> = Vec::new(); // (linear page, walk), first touch order
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut i = 0u64;
    let mut src_walk = usize::MAX;
    let mut dst_walk = usize::MAX;
    let mut src_page = u64::MAX;
    let mut dst_page = u64::MAX;
    while i < n {
        let d = rdi.wrapping_add(i * size);
        let mut left = (0x1000 - (d & 0xfff)).div_ceil(size);
        if movs {
            let a = rsi.wrapping_add(i * size);
            if a >> 12 != src_page {
                src_page = a >> 12;
                let Some(walk) =
                    fast_translate(env.ram, &env.paging, a & !0xfff, WalkKind::Read, env.user())
                else {
                    return 1;
                };
                walks.push((src_page, walk));
                src_walk = walks.len() - 1;
            }
            left = left.min((0x1000 - (a & 0xfff)).div_ceil(size));
        }
        if d >> 12 != dst_page {
            dst_page = d >> 12;
            let Some(walk) = fast_translate(
                env.ram,
                &env.paging,
                d & !0xfff,
                WalkKind::Write,
                env.user(),
            ) else {
                return 1;
            };
            walks.push((dst_page, walk));
            dst_walk = walks.len() - 1;
        }
        let count = left.min(n - i);
        chunks.push(Chunk {
            count,
            src: src_walk,
            dst: dst_walk,
        });
        i += count;
    }
    engine.stats.rep_pages += walks.len() as u64;
    // The table frames of the walks: no element may read or write one of them while an
    // accessed/dirty bit of the walks is still to be set (the general path interleaves them), and
    // none may be written at all (the translation would change between elements).
    let mut tables: Vec<u64> = Vec::new();
    for (_, walk) in &walks {
        for table in walk.tables.iter().flatten() {
            tables.push(table.get() >> 12);
        }
    }
    let mut ad_pending = false;
    for (index, (_, walk)) in walks.iter().enumerate() {
        let write = chunks.iter().any(|chunk| chunk.dst == index);
        if walk.needs_ad(write) {
            ad_pending = true;
        }
    }
    let mut table_writes: Vec<u64> = Vec::new();
    let mut code_writes: Vec<(u64, u64, u64)> = Vec::new(); // (frame, start, end) in the page
    for chunk in &chunks {
        let frame = walks[chunk.dst].1.physical >> 12;
        if tables.contains(&frame) {
            return 1;
        }
        if movs && ad_pending && tables.contains(&(walks[chunk.src].1.physical >> 12)) {
            return 1;
        }
        if engine.pt_frames.contains_key(&frame) && !table_writes.contains(&frame) {
            table_writes.push(frame);
        }
    }
    for (index, (_, walk)) in walks.iter().enumerate() {
        let write = chunks.iter().any(|chunk| chunk.dst == index);
        walk.apply(write);
    }
    // The elements.
    let accumulator = state.gpr[Register::Rax as usize] & width_mask(size);
    let mut i = 0u64;
    for chunk in &chunks {
        let d = rdi.wrapping_add(i * size);
        let dst_walk = &walks[chunk.dst].1;
        // SAFETY: the chunk stays inside the destination's host page.
        let dst = unsafe { dst_walk.host_page.add((d & 0xfff) as usize) };
        let bytes = (chunk.count * size) as usize;
        let frame = dst_walk.physical >> 12;
        if engine.code_bytes.contains_key(&frame)
            || env.ram.code_watch(GuestPhysicalAddress::new(frame << 12))
        {
            code_writes.push((frame, d & 0xfff, (d & 0xfff) + bytes as u64));
        }
        if movs {
            let a = rsi.wrapping_add(i * size);
            // SAFETY: as above for the source page.
            let src = unsafe { walks[chunk.src].1.host_page.add((a & 0xfff) as usize) };
            let (s0, d0) = (src as usize, dst as usize);
            if d0 >= s0 + bytes || s0 >= d0 + bytes || d0 <= s0 {
                // Disjoint, or the destination below the source: a forward copy is exact.
                // SAFETY: both ranges inside their host pages.
                unsafe { std::ptr::copy(src, dst, bytes) };
            } else {
                // The destination overlaps ahead of the source: element by element.
                for e in 0..chunk.count as usize {
                    // SAFETY: as above.
                    unsafe {
                        std::ptr::copy(
                            src.add(e * size as usize),
                            dst.add(e * size as usize),
                            size as usize,
                        )
                    };
                }
            }
        } else {
            // SAFETY: inside the destination's host page.
            unsafe {
                match size {
                    1 => std::ptr::write_bytes(dst, accumulator as u8, bytes),
                    2 => {
                        for e in 0..chunk.count as usize {
                            dst.add(e * 2)
                                .cast::<u16>()
                                .write_unaligned(accumulator as u16);
                        }
                    }
                    4 => {
                        for e in 0..chunk.count as usize {
                            dst.add(e * 4)
                                .cast::<u32>()
                                .write_unaligned(accumulator as u32);
                        }
                    }
                    _ => {
                        for e in 0..chunk.count as usize {
                            dst.add(e * 8).cast::<u64>().write_unaligned(accumulator);
                        }
                    }
                }
            }
        }
        i += chunk.count;
    }
    state.gpr[Register::Rsi as usize] = if movs {
        rsi.wrapping_add(n * size)
    } else {
        rsi
    };
    state.gpr[Register::Rdi as usize] = rdi.wrapping_add(n * size);
    state.gpr[Register::Rcx as usize] = count - n;
    let changed = !table_writes.is_empty() || !code_writes.is_empty();
    for frame in table_writes {
        engine.table_written(env.ram, frame);
    }
    for (frame, start, end) in code_writes {
        engine.note_code_write(env.ram, (frame << 12) + start, end - start);
        // A page the engine holds no code of any more stays watched by memory otherwise.
    }
    if count > limit {
        return 2;
    }
    state.rip = rip.wrapping_add(length);
    if changed { 4 } else { 0 }
}

/// S7-32: REPE CMPS of generated code (forward, 64-bit addresses, `size` bytes per element) as the
/// general path runs it: per element the read at RSI, then the one at RDI, the compare, RSI/RDI
/// forward, RCX down; up to the end of the count, the first unequal pair or
/// `REP_ITERATION_WORK_LIMIT` elements. The iterations run on copies of the registers; every page
/// is reached through the fast walk on its first touch (its accessed bits, as the general path's
/// walk sets them) and must be RAM. Status: bit 0 refused (the registers are untouched; only
/// accessed bits the general path sets again on the same pages in the same order may have been
/// set: the general step runs the instruction); bit 1 the limit was reached (registers moved, not
/// retired); bit 2 an element was compared (`cmp_a`/`cmp_b` of the context hold the last pair,
/// the template sets the flags from them). Without bit 1 the instruction retired (RIP is after
/// it); a zero count compares nothing (status 0).
unsafe extern "C" fn cmps_helper(
    ctx: *mut TurboCtx,
    state: *mut LongModeState,
    size: u64,
    length: u64,
    rip: u64,
) -> u64 {
    // SAFETY: called only from generated code during `TurboEngine::run`, which set `env`.
    let ctx = unsafe { &mut *ctx };
    let env = unsafe { &mut *ctx.env };
    let engine = unsafe { &mut *env.engine };
    let state = unsafe { &mut *state };
    engine.stats.cmps_calls += 1;
    const DF: u64 = 1 << 10;
    if state.rflags & DF != 0 {
        engine.refusal = Some(Refusal::Rep);
        return 1;
    }
    use od_isa::Register;
    let mut count = state.gpr[Register::Rcx as usize];
    if count == 0 {
        state.rip = rip.wrapping_add(length);
        return 0;
    }
    let mut rsi = state.gpr[Register::Rsi as usize];
    let mut rdi = state.gpr[Register::Rdi as usize];
    // The host pages of the last source and destination pages (linear page, host page).
    let mut pages = [(u64::MAX, std::ptr::null::<u8>()); 2];
    let host = |pages: &mut [(u64, *const u8); 2], slot: usize, address: u64| {
        if (address & 0xfff) + size > 0x1000 {
            return None;
        }
        if pages[slot].0 != address >> 12 {
            let walk = fast_translate(env.ram, &env.paging, address, WalkKind::Read, env.user())?;
            apply_ad(&walk, false);
            pages[slot] = (address >> 12, walk.host_page);
        }
        // SAFETY: inside the host page of the element's page (no crossing, checked above).
        Some(unsafe { pages[slot].1.add((address & 0xfff) as usize) })
    };
    let read = |at: *const u8| -> u64 {
        // SAFETY: `size` bytes inside one host RAM page.
        unsafe {
            match size {
                1 => u64::from(*at),
                2 => u64::from(at.cast::<u16>().read_unaligned()),
                4 => u64::from(at.cast::<u32>().read_unaligned()),
                _ => at.cast::<u64>().read_unaligned(),
            }
        }
    };
    let limit = od_isa::execute::REP_ITERATION_WORK_LIMIT;
    let mut completed = 0;
    let (mut a, mut b);
    loop {
        let Some(source) = host(&mut pages, 0, rsi) else {
            engine.refusal = Some(Refusal::Rep);
            return 1;
        };
        a = read(source);
        let Some(destination) = host(&mut pages, 1, rdi) else {
            engine.refusal = Some(Refusal::Rep);
            return 1;
        };
        b = read(destination);
        rsi = rsi.wrapping_add(size);
        rdi = rdi.wrapping_add(size);
        count -= 1;
        completed += 1;
        if count == 0 || a != b {
            break;
        }
        if completed == limit {
            break;
        }
    }
    state.gpr[Register::Rsi as usize] = rsi;
    state.gpr[Register::Rdi as usize] = rdi;
    state.gpr[Register::Rcx as usize] = count;
    ctx.cmp_a = a;
    ctx.cmp_b = b;
    if count != 0 && a == b {
        // The limit: registers moved, the instruction did not retire.
        return 2 | 4;
    }
    state.rip = rip.wrapping_add(length);
    4
}

fn width_mask(size: u64) -> u64 {
    match size {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

/// The host address of an access of `len` bytes at `address` with page privilege `privilege`
/// (`AccessContext::page_privilege`) through the view's TLB (`write`: the write TLB) when the
/// view's context is the one of `paging` and of that privilege: an entry of a CPL0 context serves
/// only an access of page privilege `Ring0` (the implicit descriptor-table accesses at CPL3
/// included), an entry of a CPL3 context only a user access (ADR-0023 decision 7: "`view_host`
/// takes the CPL into account"). RFLAGS.AC does not change a translation and is not compared.
#[inline(always)]
pub(crate) fn view_host(
    view: *const TlbView,
    paging: &Long64ProbeConfig,
    privilege: PrivilegeLevel,
    write: bool,
    address: u64,
    len: usize,
) -> Option<*mut u8> {
    // SAFETY: the view of the live engine.
    let view = unsafe { &*view };
    if view.paging.as_ref() != Some(paging) || view.privilege != privilege {
        return None;
    }
    tlb_host(if write { view.wtlb } else { view.rtlb }, address, len)
}

/// The host address of an access of `len` bytes at `address` when `tlb` has its page.
pub(crate) fn tlb_host(tlb: *const TlbEntry, address: u64, len: usize) -> Option<*mut u8> {
    let page = address >> 12;
    if (address & 0xfff) + len as u64 > 0x1000 {
        return None;
    }
    // SAFETY: `tlb` points at the TLB_SIZE entries of the live engine.
    let entry = unsafe { *tlb.add((page as usize) & (TLB_SIZE - 1)) };
    (entry.tag == page).then(|| address.wrapping_add(entry.addend) as *mut u8)
}

/// RDTSC of generated code at `rip` (bare `0F 31`), `remaining` instructions of budget left before
/// it. It retires here (the source of the set loop gives the value and raises the tick), which ends
/// the quantum; the board's fast transition then starts the next one at the boundary after it, and
/// that boundary polls the interrupt source as the loop would. Returns the budget of the new
/// quantum (the generated code goes on), or -1 (the boundary belongs to the loop: an interrupt to
/// deliver, or no fast transition), or -3 (no source: the general path runs the instruction).
unsafe extern "C" fn rdtsc_helper(
    ctx: *mut TurboCtx,
    state: *mut LongModeState,
    rip: u64,
    remaining: i64,
) -> i64 {
    // SAFETY: called only from generated code during `TurboEngine::run`.
    let ctx = unsafe { &mut *ctx };
    let env = unsafe { &mut *ctx.env };
    let state = unsafe { &mut *state };
    let Some(source) = env.rdtsc else {
        return -3;
    };
    // SAFETY: the engine of the run.
    unsafe { (*env.engine).stats.rdtsc_calls += 1 };
    use od_isa::Register;
    let value = match (env.tsc_next.take(), env.tick) {
        (Some(tsc), Some(tick)) => {
            // SAFETY: the board's tick flag, alive for the run.
            unsafe { (*tick).store(true, std::sync::atomic::Ordering::Relaxed) };
            tsc
        }
        // SAFETY: the source closure of the set loop, alive for the run.
        _ => unsafe { (*source)() },
    };
    state.gpr[Register::Rax as usize] = u64::from(value as u32);
    state.gpr[Register::Rdx as usize] = u64::from((value >> 32) as u32);
    state.rip = rip.wrapping_add(2);
    // The segment that ends with this RDTSC.
    let segment = (ctx.seg_start - remaining) as u64 + 1;
    if ctx.masked != 0 && segment > 1 {
        if let Some(irq) = env.irq {
            // SAFETY: the hook of the set loop.
            unsafe { (*irq).counters.irq_refused_masked += segment - 1 };
        }
    }
    ctx.run_retired += segment;
    // Nothing of this segment is left to count at the end of the run.
    ctx.seg_start = remaining - 1;
    let executed = env.executed_at_start + ctx.run_retired;
    let Some(fast) = env.fast else {
        return -1;
    };
    // SAFETY: the loop's locals, alive for the run.
    let quantum_base = unsafe { &mut *env.quantum_base };
    let instruction_budget = unsafe { &mut *env.instruction_budget };
    if executed <= *quantum_base {
        return -1;
    }
    let irq = env.irq.map(|irq| unsafe { &mut *irq });
    let start = {
        let source = irq
            .as_ref()
            .map(|hook| &*hook.source as &dyn crate::irq_source::BoardIrqSource);
        // SAFETY: the board's closure, alive for the run.
        unsafe {
            (*fast)(
                FastQuantumEnd {
                    retired: executed - *quantum_base,
                    cpl: env.cpl,
                },
                source,
            )
        }
    };
    let Some(start) = start else {
        return -1;
    };
    *quantum_base = executed;
    *instruction_budget = executed + start.budget;
    // SAFETY: the engine of the run.
    unsafe { (*env.engine).stats.fast_transitions += 1 };
    env.tsc_next = Some(start.tsc);
    let mut masked = false;
    if let Some(hook) = irq {
        if start.fresh_source {
            hook.polled = None;
        }
        hook.boundary_flags &= !crate::irq_source::BOUNDARY_SKIP_FIRST;
        if start.cut_by_deadline {
            hook.counters.quantum_cut_by_deadline += 1;
        }
        // The first boundary of the new quantum.
        if crate::paged_memory_adapter::poll_pending_irq(hook, state).is_some() {
            // Deliverable: the loop delivers it at this boundary (its poll answers the same).
            hook.polled = None;
            ctx.seg_start = start.budget as i64;
            ctx.masked = 0;
            return -2;
        }
        masked = hook.polled.is_some_and(|polled| polled.vector.is_some())
            && state.rflags & (1 << 9) == 0;
    }
    ctx.masked = masked as u64;
    ctx.seg_start = start.budget as i64;
    start.budget as i64
}

/// DIV/IDIV of generated code: RDX:RAX (EDX:EAX) by `divisor`; params: size | 0x100 signed.
/// Returns 0 (done) or 1 (#DE: the general path raises it; nothing changed).
unsafe extern "C" fn div_helper(state: *mut LongModeState, divisor: u64, params: u64) -> u64 {
    // SAFETY: called only from generated code during a run.
    let state = unsafe { &mut *state };
    use od_isa::Register;
    let size = params & 0xff;
    let signed = params & 0x100 != 0;
    let rax = state.gpr[Register::Rax as usize];
    let rdx = state.gpr[Register::Rdx as usize];
    if size == 8 {
        if signed {
            let dividend = ((u128::from(rdx) << 64) | u128::from(rax)) as i128;
            let divisor = divisor as i64 as i128;
            if divisor == 0 {
                return 1;
            }
            let Some(quotient) = dividend.checked_div(divisor) else {
                return 1;
            };
            if quotient > i128::from(i64::MAX) || quotient < i128::from(i64::MIN) {
                return 1;
            }
            let remainder = dividend - quotient * divisor;
            state.gpr[Register::Rax as usize] = quotient as i64 as u64;
            state.gpr[Register::Rdx as usize] = remainder as i64 as u64;
        } else {
            let dividend = (u128::from(rdx) << 64) | u128::from(rax);
            if divisor == 0 {
                return 1;
            }
            let quotient = dividend / u128::from(divisor);
            if quotient > u128::from(u64::MAX) {
                return 1;
            }
            state.gpr[Register::Rax as usize] = quotient as u64;
            state.gpr[Register::Rdx as usize] = (dividend % u128::from(divisor)) as u64;
        }
    } else {
        let low = rax & 0xffff_ffff;
        let high = rdx & 0xffff_ffff;
        let divisor = divisor & 0xffff_ffff;
        if signed {
            let dividend = ((high << 32) | low) as i64;
            let divisor = divisor as u32 as i32 as i64;
            if divisor == 0 {
                return 1;
            }
            let Some(quotient) = dividend.checked_div(divisor) else {
                return 1;
            };
            if quotient > i64::from(i32::MAX) || quotient < i64::from(i32::MIN) {
                return 1;
            }
            let remainder = dividend - quotient * divisor;
            state.gpr[Register::Rax as usize] = u64::from(quotient as i32 as u32);
            state.gpr[Register::Rdx as usize] = u64::from(remainder as i32 as u32);
        } else {
            let dividend = (high << 32) | low;
            if divisor == 0 {
                return 1;
            }
            let quotient = dividend / divisor;
            if quotient > u64::from(u32::MAX) {
                return 1;
            }
            state.gpr[Register::Rax as usize] = quotient;
            state.gpr[Register::Rdx as usize] = dividend % divisor;
        }
    }
    0
}

/// STI of generated code (bare `FB` at CPL0): IF = 1, and the one-instruction shadow when IF was
/// 0; 0 returned. In the shadow of another STI (the one instruction the engine runs there) nothing
/// changes and 1 is returned: the general step owns that case (its profile refuses it).
unsafe extern "C" fn sti_helper(state: *mut LongModeState) -> u64 {
    // SAFETY: called only from generated code during a run.
    let state = unsafe { &mut *state };
    if state.sti_shadow != Some(od_isa::StiShadow::Clear) {
        return 1;
    }
    let was_enabled = state.rflags & (1 << 9) != 0;
    state.rflags |= 1 << 9;
    state.sti_shadow = Some(if was_enabled {
        od_isa::StiShadow::Clear
    } else {
        od_isa::StiShadow::Blocking
    });
    0
}

/// Admission of one instruction of generated code in the STI shadow (it clears after it).
pub(crate) fn admits_in_shadow(state: &LongModeState) -> bool {
    const TF: u64 = 1 << 8;
    const RF: u64 = 1 << 16;
    state.rflags & (TF | RF) == 0
        && state.sti_shadow == Some(od_isa::StiShadow::Blocking)
        && state
            .activity
            .is_none_or(|activity| activity == od_isa::CpuActivity::Running)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkKind {
    Read,
    Write,
    Fetch,
}

/// A successful read-only translation (see [`fast_translate`]).
pub(crate) struct FastWalk {
    /// The physical address of the linear one.
    pub physical: u64,
    /// The table frames of the path (PML4 first; `None` past a 2 MiB leaf).
    pub tables: [Option<GuestPhysicalAddress>; 4],
    entries: [*mut u64; 4],
    values: [u64; 4],
    count: usize,
    /// The host address of the 4 KiB page of `physical`.
    pub host_page: *mut u8,
}

const PTE_PRESENT: u64 = 1;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_USER: u64 = 1 << 2;
const PTE_ACCESSED: u64 = 1 << 5;
const PTE_DIRTY: u64 = 1 << 6;
const PTE_PS: u64 = 1 << 7;
const PTE_NX: u64 = 1 << 63;
const PTE_ADDRESS_RESERVED: u64 = ((1u64 << 16) - 1) << 36;
const PTE_ADDRESS_MASK: u64 = ((1u64 << 36) - 1) & !0xfff;
const PTE_2M_RESERVED: u64 = 0x1fe000;

/// The translation of `linear` for an access of `kind` of CPL0 (`user == false`) or CPL3
/// (`user == true`) with the checks of the general path's walker, reading the tables through host
/// pointers and writing nothing; `None` for anything the fast form does not cover (a fault, a
/// table outside RAM, an operand outside RAM/framebuffer): the general path's walker decides
/// those. A user access needs U/S = 1 at every level, a user write R/W = 1 at every level
/// regardless of CR0.WP (ADR-0023 decision 7; the walker's `user_denied` and `write_denied`). On
/// success [`apply_ad`] makes the accessed and dirty bits that walker sets.
pub(crate) fn fast_translate(
    ram: &PhysicalMemory,
    paging: &Long64ProbeConfig,
    linear: u64,
    kind: WalkKind,
    user: bool,
) -> Option<FastWalk> {
    if ((linear as i64) >> 47) != 0 && ((linear as i64) >> 47) != -1 {
        return None;
    }
    let mut base = paging.root();
    let mut walk = FastWalk {
        physical: 0,
        tables: [None; 4],
        entries: [std::ptr::null_mut(); 4],
        values: [0; 4],
        count: 0,
        host_page: std::ptr::null_mut(),
    };
    let nxe = paging.nxe();
    let mut writable = true;
    let mut user_ok = true;
    let mut no_execute = false;
    let mut large = false;
    let mut leaf = 0;
    for level in 0..4usize {
        let table = ram.ram_only_page_host_ptr(GuestPhysicalAddress::new(base))?;
        walk.tables[level] = Some(GuestPhysicalAddress::new(base));
        let index = (linear >> (39 - 9 * level)) & 0x1ff;
        // SAFETY: an aligned u64 entry inside a host RAM page.
        let entry = unsafe { table.add((index * 8) as usize).cast::<u64>() };
        let value = unsafe {
            std::sync::atomic::AtomicU64::from_ptr(entry).load(std::sync::atomic::Ordering::Relaxed)
        };
        if value & PTE_PRESENT == 0 {
            return None;
        }
        let mut offending = value & PTE_ADDRESS_RESERVED;
        if !nxe {
            offending |= value & PTE_NX;
        }
        if level < 2 && value & PTE_PS != 0 {
            offending |= PTE_PS;
        }
        let is_large = level == 2 && value & PTE_PS != 0;
        if is_large {
            offending |= value & PTE_2M_RESERVED;
        }
        if offending != 0 {
            return None;
        }
        walk.entries[level] = entry;
        walk.values[level] = value;
        walk.count = level + 1;
        writable &= value & PTE_WRITABLE != 0;
        user_ok &= value & PTE_USER != 0;
        no_execute |= value & PTE_NX != 0;
        leaf = value;
        if is_large {
            large = true;
            break;
        }
        base = value & PTE_ADDRESS_MASK;
    }
    if user && !user_ok {
        return None;
    }
    if kind == WalkKind::Write && !writable && (user || paging.write_protect()) {
        return None;
    }
    if kind == WalkKind::Fetch && nxe && no_execute {
        return None;
    }
    let offset_mask = if large { 0x1f_ffff } else { 0xfff };
    walk.physical = (leaf & PTE_ADDRESS_MASK & !offset_mask) | (linear & offset_mask);
    let page = GuestPhysicalAddress::new(walk.physical & !0xfff);
    walk.host_page = if kind == WalkKind::Fetch {
        ram.ram_only_page_host_ptr(page)?
    } else {
        ram.ram_page_host_ptr(page)?
    };
    Some(walk)
}

impl FastWalk {
    fn needs_ad(&self, write: bool) -> bool {
        (0..self.count).any(|level| {
            let leaf = level + 1 == self.count;
            let wanted = if leaf && write {
                PTE_ACCESSED | PTE_DIRTY
            } else {
                PTE_ACCESSED
            };
            self.values[level] & wanted != wanted
        })
    }
    fn apply(&self, write: bool) {
        apply_ad(self, write);
    }
}

/// The accessed bits of every level (and with `write` the dirty bit of the leaf) that the general
/// path's walker sets on the successful walk `walk`.
pub(crate) fn apply_ad(walk: &FastWalk, write: bool) {
    for level in 0..walk.count {
        let value = walk.values[level];
        let leaf = level + 1 == walk.count;
        let wanted = if leaf && write {
            PTE_ACCESSED | PTE_DIRTY
        } else {
            PTE_ACCESSED
        };
        if value & wanted != wanted {
            // OR into the current value, not the walk's snapshot: another walk of the same
            // helper may have set bits of the same entry since (a 2 MiB leaf shared by the
            // source and the destination of a REP MOVS), as the general walker's
            // compare-and-replace keeps them.
            // SAFETY: the entry pointer of the walk, an aligned u64 in host RAM.
            unsafe {
                std::sync::atomic::AtomicU64::from_ptr(walk.entries[level])
                    .fetch_or(wanted, std::sync::atomic::Ordering::AcqRel);
            }
        }
    }
}
