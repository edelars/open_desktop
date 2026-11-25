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

pub(crate) mod a64;
mod jit;

use crate::page_walk::{Long64ProbeConfig, RamAccessOperation, TrackedAccessRequest};
use crate::{GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, PhysicalMemory};
use od_isa::memory::{AccessContext, AccessKind};
use od_isa::LongModeState;
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
/// The instruction before retired and wrote a watched page: leave the run at the next one.
pub(crate) const EXIT_FLUSH: u64 = 5;
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

/// One cached translation context (TLBs and the lookup table of one CR3/paging key).
struct Slot {
    key: Option<(Long64ProbeConfig, u64)>,
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
            lookup: vec![LookupEntry { rip: 1, code: 0 }; LOOKUP_SIZE].into_boxed_slice(),
            lookup_page: vec![u64::MAX; LOOKUP_SIZE].into_boxed_slice(),
            lookup_leaf: vec![u64::MAX; LOOKUP_SIZE].into_boxed_slice(),
            stamp: 0,
        }
    }
    fn flush(&mut self) {
        self.rtlb.fill(EMPTY_TLB);
        self.wtlb.fill(EMPTY_TLB);
        self.lookup.fill(LookupEntry { rip: 1, code: 0 });
    }
    fn drop_leaf(&mut self, frame: u64) {
        for entry in self.rtlb.iter_mut().chain(self.wtlb.iter_mut()) {
            if entry.leaf == frame {
                *entry = EMPTY_TLB;
            }
        }
        for (entry, leaf) in self.lookup.iter_mut().zip(self.lookup_leaf.iter()) {
            if *leaf == frame {
                *entry = LookupEntry { rip: 1, code: 0 };
            }
        }
    }
}

/// Cached translation contexts besides the active one.
const SLOTS: usize = 3;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LookupEntry {
    pub rip: u64,
    pub code: u64,
}

/// What generated code reads and writes through x21.
#[repr(C)]
pub(crate) struct TurboCtx {
    pub budget: i64,         // 0
    pub nzcv: u64,           // 8
    pub pf: u64,             // 16
    pub af: u64,             // 24
    pub defined: u64,        // 32
    pub rtlb: *mut TlbEntry, // 40
    pub wtlb: *mut TlbEntry, // 48
    pub lookup: *mut LookupEntry, // 56
    pub mem_helper: usize,   // 64
    pub env: *mut RunEnv<'static>, // 72
    pub status: u64,         // 80
    pub rep_helper: usize,   // 88
    /// The visible ES, CS, SS, DS, FS, GS selectors (bit 16: modelled), copied at the start of a run.
    pub selectors: [u32; 6], // 96
    pub rdtsc_helper: usize, // 120
    /// The budget at the start of the current segment (after the last RDTSC transition).
    pub seg_start: i64,
    /// Instructions retired by the run in the segments before the current one.
    pub run_retired: u64,
    /// The interrupt source has a vector held back by IF = 0 (every boundary counts a refusal).
    pub masked: u64,
    pub div_helper: usize, // 152
    pub sti_helper: usize, // 160
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

/// What a run needs from the set loop (borrowed for the duration of one run).
pub(crate) struct RunEnv<'a> {
    pub ram: &'a PhysicalMemory,
    pub paging: Long64ProbeConfig,
    pub context: AccessContext,
    /// Set by the memory helper when a data write hit a page the engine watches.
    pub flush_pending: bool,
    pub engine: *mut TurboEngine,
    /// The RDTSC source of the set loop.
    pub rdtsc: Option<*mut (dyn FnMut() -> u64 + 'static)>,
    /// The board's fast transition between RDTSC quanta.
    pub fast: Option<
        *mut (dyn FnMut(FastQuantumEnd, Option<&dyn crate::irq_source::BoardIrqSource>) -> Option<FastQuantumStart>
             + 'static),
    >,
    /// The interrupt hook of the set loop.
    pub irq: Option<*mut crate::irq_source::IrqBoundaryHook<'static>>,
    /// The set loop's retired count when the run started.
    pub executed_at_start: u64,
    /// The set loop's `quantum_base` and `instruction_budget`.
    pub quantum_base: *mut u64,
    pub instruction_budget: *mut u64,
    pub cpl: usize,
    pub tick: Option<*const std::sync::atomic::AtomicBool>,
    /// The TSC an RDTSC of the current (merged) quantum reads, when the fast transition knows it.
    pub tsc_next: Option<u64>,
}

#[repr(C)]
pub(crate) struct HelperResult {
    pub value: u64,
    pub status: u64,
}

/// The executable region (one MAP_JIT mapping, bump allocated).
struct CodeArena {
    base: *mut u8,
    size: usize,
    used: usize,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod sys {
    use std::ffi::c_void;
    unsafe extern "C" {
        pub fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        pub fn pthread_jit_write_protect_np(enabled: i32);
        pub fn sys_icache_invalidate(start: *mut c_void, len: usize);
    }
    pub const PROT_READ: i32 = 1;
    pub const PROT_WRITE: i32 = 2;
    pub const PROT_EXEC: i32 = 4;
    pub const MAP_PRIVATE: i32 = 0x0002;
    pub const MAP_ANON: i32 = 0x1000;
    pub const MAP_JIT: i32 = 0x0800;
}

impl CodeArena {
    fn new(size: usize) -> Option<Self> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            // SAFETY: an anonymous private MAP_JIT mapping.
            let raw = unsafe {
                sys::mmap(
                    std::ptr::null_mut(),
                    size,
                    sys::PROT_READ | sys::PROT_WRITE | sys::PROT_EXEC,
                    sys::MAP_PRIVATE | sys::MAP_ANON | sys::MAP_JIT,
                    -1,
                    0,
                )
            };
            if raw as isize == -1 || raw.is_null() {
                return None;
            }
            Some(Self {
                base: raw.cast(),
                size,
                used: 0,
            })
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = size;
            None
        }
    }

    fn reserve(&self, words: usize) -> Option<usize> {
        let bytes = words * 4;
        if self.used + bytes > self.size {
            return None;
        }
        Some(self.base as usize + self.used)
    }

    /// Copy `words` to `at` (from `reserve`) and advance.
    fn commit(&mut self, at: usize, words: &[u32]) {
        let bytes = words.len() * 4;
        debug_assert_eq!(at, self.base as usize + self.used);
        // SAFETY: `at..at+bytes` is inside the mapping; write protection is lifted for this thread
        // only while copying, and the instruction cache is invalidated before execution.
        unsafe {
            sys::pthread_jit_write_protect_np(0);
            std::ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), at as *mut u8, bytes);
            sys::pthread_jit_write_protect_np(1);
            sys::sys_icache_invalidate(at as *mut _, bytes);
        }
        self.used += bytes;
        // keep 16-byte alignment of the next block
        self.used = (self.used + 15) & !15;
    }

    /// Overwrite one word (used to kill a block's entry).
    fn patch(&mut self, at: usize, word: u32) {
        // SAFETY: as in `commit`, one aligned word inside the mapping.
        unsafe {
            sys::pthread_jit_write_protect_np(0);
            (at as *mut u32).write(word);
            sys::pthread_jit_write_protect_np(1);
            sys::sys_icache_invalidate(at as *mut _, 4);
        }
    }

    fn reset(&mut self, keep: usize) {
        self.used = keep;
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
    stamp: u64,
    blocks: HashMap<(u64, u64), BlockInfo>,
    /// Physical page -> keys of the blocks on it.
    code_pages: HashMap<u64, Vec<(u64, u64)>>,
    /// Physical page -> bitmap of its bytes that compiled instructions came from.
    code_bytes: HashMap<u64, Box<[u64; 64]>>,
    /// Physical pages known to be page-table frames of cached translations: bit 0 a leaf
    /// table, bit 1 an upper level.
    pt_frames: HashMap<u64, u8>,
    offsets: StateOffsets,
    paging_key: Option<(Long64ProbeConfig, u64)>,
    singles: HashMap<(u64, u64), BlockInfo>,
    /// Decodes of instructions the generated code stops before, by (RIP, physical page).
    slow_insns: HashMap<(u64, u64), od_isa::long_decode::Instruction>,
    /// Decodes made by the last translation (filled by `jit::compile_limited`).
    pub(crate) pending_decodes: Vec<(u64, od_isa::long_decode::Instruction)>,
    /// The decode of the instruction at which the last run stopped.
    slow_decode: Option<(u64, od_isa::long_decode::Instruction)>,
    /// The physical page of the code the last run was in when it stopped.
    last_page: u64,
    /// The leaf table frame of the last code translation (for `install`).
    last_leaf: u64,
    collect_slow: bool,
    debug: u64,
    debug_fb: u64,
    slow_kinds: HashMap<String, u64>,
    slow_walks: HashMap<String, u64>,
    last_cause: u8,
    pending_slow: Option<(String, u64)>,
    pub stats: TurboStats,
}

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
    pub noted_writes: u64,
    pub leaf_drops: u64,
    pub rep_refused: [u64; 12],
    pub loop_cpl3: u64,
    pub loop_unknown: u64,
    pub loop_refused: u64,
    pub loop_forced: u64,
    pub slot_switches: u64,
    pub slot_misses: u64,
    pub singles: u64,
    pub rep_calls: u64,
    pub rep_pages: u64,
    pub rdtsc_calls: u64,
    pub fast_transitions: u64,
    pub helper_reads: u64,
    pub helper_writes: u64,
}

impl TurboEngine {
    pub(crate) fn new() -> Option<Box<Self>> {
        let offsets = StateOffsets {
            gpr: std::mem::offset_of!(LongModeState, gpr) as u32,
            rip: std::mem::offset_of!(LongModeState, rip) as u32,
            rflags: std::mem::offset_of!(LongModeState, rflags) as u32,
            fs_base: std::mem::offset_of!(LongModeState, fs_base) as u32,
            gs_base: std::mem::offset_of!(LongModeState, gs_base) as u32,
        };
        // ldr/str x immediate reach: 32760.
        if offsets.gpr + 16 * 8 > 32760 || offsets.rip > 32760 || offsets.gs_base > 32760 {
            return None;
        }
        let arena = CodeArena::new(96 << 20)?;
        let mut rtlb = vec![EMPTY_TLB; TLB_SIZE].into_boxed_slice();
        let mut wtlb = vec![EMPTY_TLB; TLB_SIZE].into_boxed_slice();
        let mut lookup = vec![LookupEntry { rip: 1, code: 0 }; LOOKUP_SIZE].into_boxed_slice();
        let ctx = Box::new(TurboCtx {
            budget: 0,
            nzcv: 0,
            pf: 0,
            af: 0,
            defined: 0,
            rtlb: rtlb.as_mut_ptr(),
            wtlb: wtlb.as_mut_ptr(),
            lookup: lookup.as_mut_ptr(),
            mem_helper: mem_helper as usize,
            env: std::ptr::null_mut(),
            status: 0,
            rep_helper: rep_helper as usize,
            selectors: [0; 6],
            rdtsc_helper: rdtsc_helper as usize,
            seg_start: 0,
            run_retired: 0,
            masked: 0,
            div_helper: div_helper as usize,
            sti_helper: sti_helper as usize,
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
            stamp: 0,
            blocks: HashMap::default(),
            code_pages: HashMap::default(),
            code_bytes: HashMap::default(),
            pt_frames: HashMap::default(),
            offsets,
            paging_key: None,
            singles: HashMap::default(),
            slow_insns: HashMap::default(),
            slow_decode: None,
            pending_decodes: Vec::new(),
            last_page: u64::MAX,
            last_leaf: u64::MAX,
            collect_slow: std::env::var_os("OD_TURBO_STATS").is_some(),
            debug: std::env::var("OD_TURBO_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            debug_fb: std::env::var("OD_TURBO_DEBUG_FB").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            slow_kinds: HashMap::default(),
            slow_walks: HashMap::default(),
            last_cause: 0,
            pending_slow: None,
            stats: TurboStats::default(),
        });
        engine.emit_stubs();
        Some(engine)
    }

    fn emit_stubs(&mut self) {
        use a64::*;
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
        self.lookup.fill(LookupEntry { rip: 1, code: 0 });
        for slot in &mut self.slots {
            slot.lookup.fill(LookupEntry { rip: 1, code: 0 });
        }
    }

    /// Every translation the engine cached (TLBs, lookup tables, page-table watch), in every
    /// context.
    pub(crate) fn flush_translations(&mut self) {
        self.stats.flushes += 1;
        self.rtlb.fill(EMPTY_TLB);
        self.wtlb.fill(EMPTY_TLB);
        self.lookup.fill(LookupEntry { rip: 1, code: 0 });
        for slot in &mut self.slots {
            slot.flush();
        }
        self.pt_frames.clear();
    }

    /// A table frame was written: the translations through it are dropped (only those whose
    /// leaf it holds when it is a leaf table only; everything otherwise).
    fn table_written(&mut self, frame: u64) {
        match self.pt_frames.get(&frame).copied() {
            None => {}
            Some(levels) if levels & 2 != 0 => self.flush_translations(),
            Some(_) => {
                self.stats.leaf_drops += 1;
                for entry in self.rtlb.iter_mut().chain(self.wtlb.iter_mut()) {
                    if entry.leaf == frame {
                        *entry = EMPTY_TLB;
                    }
                }
                for (entry, leaf) in self.lookup.iter_mut().zip(self.lookup_leaf.iter()) {
                    if *leaf == frame {
                        *entry = LookupEntry { rip: 1, code: 0 };
                    }
                }
                for slot in &mut self.slots {
                    slot.drop_leaf(frame);
                }
                self.pt_frames.remove(&frame);
            }
        }
    }

    /// Note the table frames of a walk (PML4 first; the last one holds the leaf); `true` when
    /// one of them was not watched before.
    fn watch_tables(&mut self, tables: &[Option<GuestPhysicalAddress>; 4]) -> (bool, u64) {
        let present: Vec<u64> = tables.iter().flatten().map(|table| table.get() >> 12).collect();
        let mut new = false;
        let leaf = present.last().copied().unwrap_or(u64::MAX);
        for (index, frame) in present.iter().enumerate() {
            let bit = if index + 1 == present.len() { 1 } else { 2 };
            let levels = self.pt_frames.entry(*frame).or_insert(0);
            if *levels == 0 {
                new = true;
            }
            *levels |= bit;
        }
        (new, leaf)
    }





    /// Keep the decodes of the last translation (their bytes count as code of the page).
    fn keep_decodes(&mut self, page: u64) {
        let decodes = std::mem::take(&mut self.pending_decodes);
        let mut ranges = Vec::with_capacity(decodes.len());
        for (rip, instruction) in &decodes {
            if (rip & 0xfff) + u64::from(instruction.length) <= 0x1000 {
                self.slow_insns.insert((*rip, page), *instruction);
                ranges.push(((rip & 0xfff) as u16, instruction.length));
            }
        }
        self.mark_code(page, &ranges);
        self.code_pages.entry(page).or_default();
        self.pending_decodes = decodes;
        self.pending_decodes.clear();
    }

    fn mark_code(&mut self, page: u64, ranges: &[(u16, u8)]) {
        let map = self.code_bytes.entry(page).or_insert_with(|| Box::new([0; 64]));
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
            let mask = if take == 64 { u64::MAX } else { ((1u64 << take) - 1) << bit };
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
            let end = if page == last { ((address + len - 1) & 0xfff) + 1 } else { 4096 };
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
            }
            self.singles.retain(|_, info| info.page != page);
            self.slow_insns.retain(|key, _| key.1 != page);
            // Entries of the lookup table may point at those blocks: drop all of them.
            self.flush_lookup();
        }
        ram.set_code_watch(GuestPhysicalAddress::new(page << 12), false);
    }

    /// Writes committed outside generated code (the general path, deliveries, devices).
    pub(crate) fn note_writes(&mut self, ram: &PhysicalMemory, ranges: &[GuestPhysicalRange]) {
        self.stats.noted_writes += ranges.len() as u64;
        let mut flush = false;
        for range in ranges {
            if range.len() == 0 {
                continue;
            }
            let first = range.start().get() >> 12;
            let last = (range.start().get() + range.len() - 1) >> 12;
            for page in first..=last {
                if self.pt_frames.contains_key(&page) {
                    self.table_written(page);
                }
            }
            self.note_code_write(ram, range.start().get(), range.len());
        }
        let _ = &mut flush;
    }

    /// Code pages written by anyone (the RAM's code watch log).
    pub(crate) fn drain_code_watch(&mut self, ram: &PhysicalMemory) {
        if let Some(ranges) = ram.take_code_watch_hits() {
            for (address, len) in ranges {
                self.note_code_write(ram, address, len);
            }
        }
    }

    fn check_paging(&mut self, state: &LongModeState, paging: &Long64ProbeConfig) {
        let key = (*paging, state.rflags & (1 << 18));
        if self.paging_key == Some(key) {
            return;
        }
        self.stamp += 1;
        let index = match self.slots.iter().position(|slot| slot.key == Some(key)) {
            Some(index) => index,
            None => {
                // The least recently used context gives way.
                let index = (0..SLOTS).min_by_key(|&index| self.slots[index].stamp).unwrap_or(0);
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
    }

    fn install(&mut self, rip: u64, code: usize, page: u64) {
        let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
        self.lookup[index] = LookupEntry {
            rip,
            code: code as u64,
        };
        self.lookup_page[index] = page;
        self.lookup_leaf[index] = self.last_leaf;
    }

    /// The code for `rip`: an existing block, a fresh one, or the slow/RDTSC stub. `None` when
    /// the fetch itself needs the general path (a fault, not RAM).
    fn code_for(&mut self, env: &mut RunEnv<'_>, rip: u64) -> Option<usize> {
        self.stats.lookups += 1;
        if let Some(walk) = fast_translate(env.ram, &env.paging, rip, WalkKind::Fetch) {
            apply_ad(&walk, false);
            return self.code_for_physical(env, rip, GuestPhysicalAddress::new(walk.physical), walk.tables, walk.host_page);
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
        let (physical, tables) =
            crate::page_walk::data_translation_of_walk_trace(&report.trace, GuestLinearAddress::new(rip))?;
        env.ram.ram_frame(GuestPhysicalAddress::new(physical.get() & !0xfff))?;
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
        let (new, leaf) = self.watch_tables(&tables);
        if new {
            self.flush_write_tlb();
        }
        self.last_leaf = leaf;
        if let Some(info) = self.blocks.get(&(rip, page)) {
            let code = info.code;
            self.install(rip, code, page);
            return Some(code);
        }
        self.pending_decodes.clear();
        let compiled = jit::compile(self, rip, page, host_page as *const u8, env.context);
        self.keep_decodes(page);
        if !matches!(compiled, jit::Compiled::NoSpace) && !env.ram.code_watch(physical) {
            env.ram.set_code_watch(physical, true);
            // No write entry may point at a page with code.
            self.flush_write_tlb();
        }
        if self.debug > 0 {
            self.debug -= 1;
            let what = match &compiled {
                jit::Compiled::Block(code, n, _) => format!("block {n} at {code:#x}"),
                jit::Compiled::Slow => format!("slow: {}", jit::describe(rip, host_page as *const u8, env.context)),
                jit::Compiled::Rdtsc => "rdtsc".into(),
                jit::Compiled::NoSpace => "nospace".into(),
                jit::Compiled::Crossing => "crossing".into(),
            };
            eprintln!("turbo compile {rip:#x}: {what}");
        }
        let code = match compiled {
            jit::Compiled::Block(code, instructions, ranges) => {
                self.mark_code(page, &ranges);
                self.stats.compiled_blocks += 1;
                self.stats.compiled_instructions += instructions as u64;
                self.blocks.insert((rip, page), BlockInfo { code, page });
                self.code_pages.entry(page).or_default().push((rip, page));
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
                self.blocks.insert((rip, page), BlockInfo { code: self.slow_stub, page });
                self.code_pages.entry(page).or_default().push((rip, page));
                self.slow_stub
            }
            jit::Compiled::Rdtsc => self.rdtsc_stub,
            jit::Compiled::Crossing => match self.crossing_block(env, rip, physical) {
                Some(code) => code,
                None => self.slow_stub,
            },
            jit::Compiled::NoSpace => {
                // Start over with an empty code cache.
                self.arena.reset(self.stubs_end);
                self.blocks.clear();
                self.singles.clear();
                self.code_bytes.clear();
                self.slow_insns.clear();
                for page in self.code_pages.keys() {
                    env.ram.set_code_watch(GuestPhysicalAddress::new(page << 12), false);
                }
                self.code_pages.clear();
                self.flush_lookup();
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
        let (physical, _) =
            crate::page_walk::data_translation_of_walk_trace(&report.trace, GuestLinearAddress::new(rip))?;
        let page = physical.get() >> 12;
        if let Some(info) = self.singles.get(&(rip, page)) {
            return Some(info.code);
        }
        let host_page = env.ram.ram_page_host_ptr(physical)?;
        self.pending_decodes.clear();
        let compiled = jit::compile_limited(self, rip, page, host_page as *const u8, env.context, 1, None);
        self.keep_decodes(page);
        if !env.ram.code_watch(physical) {
            env.ram.set_code_watch(physical, true);
            self.flush_write_tlb();
        }
        match compiled {
            jit::Compiled::Block(code, _, ranges) => {
                self.mark_code(page, &ranges);
                self.singles.insert((rip, page), BlockInfo { code, page });
                self.code_pages.entry(page).or_default();
                if !env.ram.code_watch(physical) {
                    env.ram.set_code_watch(physical, true);
                    self.flush_write_tlb();
                }
                Some(code)
            }
            jit::Compiled::Slow | jit::Compiled::NoSpace => None,
            jit::Compiled::Crossing => self.crossing_block(env, rip, physical),
            jit::Compiled::Rdtsc => Some(self.rdtsc_stub_for(rip)),
        }
    }

    /// A block of the one instruction at `rip` whose bytes continue on the next page.
    fn crossing_block(&mut self, env: &mut RunEnv<'_>, rip: u64, physical: GuestPhysicalAddress) -> Option<usize> {
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
        let (new, _) = self.watch_tables(&tables);
        if new {
            self.flush_write_tlb();
        }
        // A crossing block depends on two translations: it is never in the lookup table of a
        // leaf drop it would miss, so drop it with any change of the second table too.
        if let Some(frame) = tables.iter().flatten().last() {
            self.pt_frames.insert(frame.get() >> 12, 2);
        }
        let page = physical.get() >> 12;
        let next_page = next_physical.get() >> 12;
        let host = env.ram.ram_page_host_ptr(physical)?;
        let next_host = env.ram.ram_page_host_ptr(next_physical)?;
        self.pending_decodes.clear();
        let compiled = jit::compile_limited(
            self,
            rip,
            page,
            host as *const u8,
            env.context,
            1,
            Some(next_host as *const u8),
        );
        self.pending_decodes.clear();
        let jit::Compiled::Block(code, _, _) = compiled else {
            return None;
        };
        // Both pages hold its bytes: the tail of the first, the head of the second.
        let offset = (rip & 0xfff) as u16;
        self.mark_code(page, &[(offset, (0x1000 - u32::from(offset)) as u8)]);
        self.mark_code(next_page, &[(0, 16)]);
        for (p, phys) in [(page, physical), (next_page, next_physical)] {
            self.code_pages.entry(p).or_default().push((rip, page));
            if !env.ram.code_watch(phys) {
                env.ram.set_code_watch(phys, true);
            }
        }
        self.flush_write_tlb();
        self.blocks.insert((rip, page), BlockInfo { code, page });
        self.stats.compiled_blocks += 1;
        Some(code)
    }

    fn rdtsc_stub_for(&self, _rip: u64) -> usize {
        self.rdtsc_stub
    }

    /// The engine's TLBs for the general path's adapter.
    pub(crate) fn tlbs(&self) -> (*const TlbEntry, *const TlbEntry) {
        (self.rtlb.as_ptr(), self.wtlb.as_ptr())
    }

    /// The decode of the instruction at `rip` the last run ended before, if the engine has it.
    pub(crate) fn take_slow_decode(&mut self, rip: u64) -> Option<(u64, od_isa::long_decode::Instruction)> {
        match self.slow_decode.take() {
            Some((at, instruction)) if at == rip => Some((at, instruction)),
            _ => None,
        }
    }

    /// Diagnostic (OD_TURBO_STATS): count the operation the general path runs at `rip`.
    pub(crate) fn note_slow(
        &mut self,
        ram: &PhysicalMemory,
        paging: &Long64ProbeConfig,
        context: AccessContext,
        rip: u64,
    ) {
        self.stats.general_steps += 1;
        if !self.collect_slow {
            return;
        }
        let Ok(physical) = crate::page_walk::translate_long64_2m_ram_read_only(
            ram,
            paging,
            rip,
            GuestLinearAddress::new(rip),
            context,
            AccessKind::Fetch,
        ) else {
            *self.slow_kinds.entry("fetch-fault".into()).or_default() += 1;
            return;
        };
        let Some(host) = ram.ram_page_host_ptr(physical) else {
            *self.slow_kinds.entry("fetch-not-ram".into()).or_default() += 1;
            return;
        };
        let mut key = jit::describe(rip, host as *const u8, context);
        if std::env::var_os("OD_TURBO_RIPS").is_some() {
            key = format!("{rip:#x} c{} {}", self.last_cause, &key[..key.len().min(40)]);
        }
        *self.slow_kinds.entry(key.clone()).or_default() += 1;
        self.pending_slow = Some((key, crate::page_walk::WALK_COUNT.load(std::sync::atomic::Ordering::Relaxed)));
    }

    /// Diagnostic: the walks of the general step just done.
    pub(crate) fn settle_slow(&mut self) {
        if let Some((key, before)) = self.pending_slow.take() {
            let now = crate::page_walk::WALK_COUNT.load(std::sync::atomic::Ordering::Relaxed);
            *self.slow_walks.entry(key).or_default() += now - before;
        }
    }

    /// Diagnostic: the general-path operations by count.
    pub(crate) fn report(&self) -> String {
        let mut kinds: Vec<_> = self.slow_kinds.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1));
        let mut text = format!(
            "{:?} walks {}\n",
            self.stats_summary(),
            crate::page_walk::WALK_COUNT.load(std::sync::atomic::Ordering::Relaxed)
        );
        for (key, count) in kinds.iter().take(60) {
            let walks = self.slow_walks.get(*key).copied().unwrap_or(0);
            text.push_str(&format!("{count:>12} {walks:>10} {key}\n"));
        }
        text
    }

    fn stats_summary(&self) -> TurboStats {
        self.stats
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
        self.last_cause = 0;
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
        let mut exit = TurboExit::Budget;
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
                        self.last_cause = 1;
                        exit = TurboExit::Slow;
                        break;
                    }
                }
            };
            if code == self.slow_stub {
                self.last_cause = 2;
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
                                    self.last_cause = 3;
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
                                EXIT_FLUSH => {
                                    self.flush_translations();
                                    continue;
                                }
                                other => panic!("turbo: unknown exit {other}"),
                            }
                        }
                        None => {
                            self.last_cause = 4;
                            exit = TurboExit::Slow;
                            break;
                        }
                    }
                }
                EXIT_SLOW => {
                    self.last_cause = 5;
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
                EXIT_FLUSH => {
                    // A watched page was written by the instruction before.
                    self.flush_translations();
                    self.drain_code_watch(env.ram);
                    if env.flush_pending {
                        env.flush_pending = false;
                    }
                    continue;
                }
                other => panic!("turbo: unknown exit {other}"),
            }
        }
        if env.flush_pending {
            env.flush_pending = false;
            self.flush_translations();
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
        if exit == TurboExit::Slow {
            self.stats.slow_exits += 1;
            let rip = state.rip;
            let index = ((rip ^ (rip >> 12)) as usize) & (LOOKUP_SIZE - 1);
            let known_page = (self.lookup[index].rip == rip).then(|| self.lookup_page[index]);
            self.slow_decode = if let Some(page) = known_page {
                self.slow_insns.get(&(rip, page)).copied().map(|instruction| (rip, instruction))
            } else { crate::page_walk::translate_long64_2m_ram_read_only(
                env.ram,
                &env.paging,
                rip,
                GuestLinearAddress::new(rip),
                env.context,
                AccessKind::Fetch,
            )
            .ok()
            .and_then(|physical| self.slow_insns.get(&(rip, physical.get() >> 12)).copied())
            .map(|instruction| (rip, instruction)) };
        }
        self.stats.retired += retired;
        TurboOutcome { retired, exit }
    }

    /// Fill a TLB entry from a fast walk of `address`.
    fn fill_fast(&mut self, address: u64, walk: &FastWalk, write: bool) {
        let page = walk.physical >> 12;
        let (new_tables, leaf) = self.watch_tables(&walk.tables);
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
        if write {
            self.wtlb[index] = entry;
        } else {
            self.rtlb[index] = entry;
        }
    }

    /// Fill a TLB entry from a successful walk.
    fn fill(
        &mut self,
        ram: &PhysicalMemory,
        address: u64,
        trace: &crate::page_walk::WalkTrace,
        write: bool,
    ) {
        let Some((physical, tables)) =
            crate::page_walk::data_translation_of_walk_trace(trace, GuestLinearAddress::new(address))
        else {
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
        let (new_tables, leaf) = self.watch_tables(&tables);
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
        if write {
            self.wtlb[index] = entry;
        } else {
            self.rtlb[index] = entry;
        }
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
/// besides accessed/dirty bits the general path sets again); 2: done, and the write hit a watched
/// page (the run must end after this instruction).
unsafe extern "C" fn mem_helper(ctx: *mut TurboCtx, address: u64, op: u64, value: u64, rip: u64) -> HelperResult {
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
    let fallback = HelperResult { value: 0, status: 1 };
    if (address & 0xfff) + size > 0x1000 {
        engine.stats.tlb_fallbacks += 1;
        return fallback;
    }
    let width = width_of(size);
    // The fast form of the walk: read-only checks, then the accessed/dirty bits and the access.
    let kind = if write { WalkKind::Write } else { WalkKind::Read };
    if let Some(walk) = fast_translate(env.ram, &env.paging, address, kind) {
        let page = walk.physical >> 12;
        // SAFETY: the host page of the walk holds the whole access (no page crossing).
        let host = unsafe { walk.host_page.add((address & 0xfff) as usize) };
        if op & OP_ATOMIC != 0 {
            if engine.pt_frames.contains_key(&page)
                || engine.code_bytes.contains_key(&page)
                || walk.tables.iter().flatten().any(|table| table.get() >> 12 == page)
            {
                engine.stats.tlb_fallbacks += 1;
                return fallback;
            }
            apply_ad(&walk, true);
            engine.fill_fast(address, &walk, true);
            engine.fill_fast(address, &walk, false);
            let linear_page = address >> 12;
            if engine.wtlb[(linear_page as usize) & (TLB_SIZE - 1)].tag == linear_page {
                return HelperResult { value: 0, status: 0 };
            }
            return fallback;
        }
        if write {
            if engine.pt_frames.contains_key(&page)
                || engine.overlaps_code(page, walk.physical & 0xfff, size)
                || walk.tables.iter().flatten().any(|table| table.get() >> 12 == page)
            {
                engine.stats.tlb_fallbacks += 1;
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
            engine.fill_fast(address, &walk, true);
            return HelperResult { value: 0, status: 0 };
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
        engine.fill_fast(address, &walk, false);
        return HelperResult { value, status: 0 };
    }
    if write {
        // Classify first, without effects: a watched page goes to the general path.
        let Ok((physical, tables)) = crate::page_walk::translate_long64_2m_ram_read_only_with_tables(
            env.ram,
            &env.paging,
            rip,
            GuestLinearAddress::new(address),
            env.context,
            AccessKind::Write,
        ) else {
            engine.stats.tlb_fallbacks += 1;
            return fallback;
        };
        let page = physical.get() >> 12;
        if engine.pt_frames.contains_key(&page)
            || engine.overlaps_code(page, physical.get() & 0xfff, size)
            || env.ram.ram_page_host_ptr(physical).is_none()
            || tables.iter().flatten().any(|table| table.get() >> 12 == page)
        {
            engine.stats.tlb_fallbacks += 1;
            if engine.debug_fb > 0 {
                engine.debug_fb -= 1;
                eprintln!(
                    "turbo write refused rip {rip:#x} addr {address:#x} phys {:#x}: pt {} code {} ram {} own {}",
                    physical.get(),
                    engine.pt_frames.contains_key(&page),
                    engine.overlaps_code(page, physical.get() & 0xfff, size),
                    env.ram.ram_frame(GuestPhysicalAddress::new(physical.get() & !0xfff)).is_some(),
                    tables.iter().flatten().any(|table| table.get() >> 12 == page)
                );
            }
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
                    HelperResult { value: 0, status: 0 }
                } else {
                    fallback
                }
            }
            _ => fallback,
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
            HelperResult { value: 0, status: 0 }
        }
        other => {
            engine.stats.tlb_fallbacks += 1;
            if engine.debug_fb > 0 {
                engine.debug_fb -= 1;
                let kind = crate::page_walk::translate_long64_2m_ram_read_only(
                    env.ram,
                    &env.paging,
                    rip,
                    GuestLinearAddress::new(address),
                    env.context,
                    if write { AccessKind::Write } else { AccessKind::Read },
                )
                .map(|physical| (physical, env.ram.locate_region_kind(physical, 1)));
                eprintln!("turbo fallback rip {rip:#x} addr {address:#x} op {op:#x}: {other:?} {kind:?}");
            }
            fallback
        }
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

/// Admission of generated code: CPL0, no TF/RF, no STI/MOV SS shadow, running.
pub(crate) fn admits(state: &LongModeState) -> bool {
    const TF: u64 = 1 << 8;
    const RF: u64 = 1 << 16;
    state.rflags & (TF | RF) == 0
        && state.sti_shadow == Some(od_isa::StiShadow::Clear)
        && state.activity.is_none_or(|activity| activity == od_isa::CpuActivity::Running)
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
        engine.stats.rep_refused[1] += 1;
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
            engine.stats.rep_refused[2] += 1;
            return 1;
        }
    }
    if movs && rsi & (size - 1) != 0 && (rsi & 0xfff) + n * size > 0x1000 {
        let crosses = (0..n).any(|i| {
            let a = rsi.wrapping_add(i * size);
            (a & 0xfff) + size > 0x1000
        });
        if crosses {
            engine.stats.rep_refused[3] += 1;
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
                let Some(walk) = fast_translate(env.ram, &env.paging, a & !0xfff, WalkKind::Read) else {
                    engine.stats.rep_refused[4] += 1;
                    return 1;
                };
                walks.push((src_page, walk));
                src_walk = walks.len() - 1;
            }
            left = left.min((0x1000 - (a & 0xfff)).div_ceil(size));
        }
        if d >> 12 != dst_page {
            dst_page = d >> 12;
            let Some(walk) = fast_translate(env.ram, &env.paging, d & !0xfff, WalkKind::Write) else {
                engine.stats.rep_refused[5] += 1;
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
            engine.stats.rep_refused[6] += 1;
            return 1;
        }
        if movs && ad_pending && tables.contains(&(walks[chunk.src].1.physical >> 12)) {
            engine.stats.rep_refused[7] += 1;
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
        if engine.code_bytes.contains_key(&frame) || env.ram.code_watch(GuestPhysicalAddress::new(frame << 12)) {
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
                        std::ptr::copy(src.add(e * size as usize), dst.add(e * size as usize), size as usize)
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
                            dst.add(e * 2).cast::<u16>().write_unaligned(accumulator as u16);
                        }
                    }
                    4 => {
                        for e in 0..chunk.count as usize {
                            dst.add(e * 4).cast::<u32>().write_unaligned(accumulator as u32);
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
    state.gpr[Register::Rsi as usize] = if movs { rsi.wrapping_add(n * size) } else { rsi };
    state.gpr[Register::Rdi as usize] = rdi.wrapping_add(n * size);
    state.gpr[Register::Rcx as usize] = count - n;
    let changed = !table_writes.is_empty() || !code_writes.is_empty();
    for frame in table_writes {
        engine.table_written(frame);
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

fn width_mask(size: u64) -> u64 {
    match size {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

/// The host address of an access of `len` bytes at `address` when `tlb` has its page.
#[inline(always)]
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
        hook.skip_first_boundary = false;
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

/// STI of generated code (bare `FB` at CPL0, the shadow clear before it): IF = 1, and the
/// one-instruction shadow when IF was 0.
unsafe extern "C" fn sti_helper(state: *mut LongModeState) {
    // SAFETY: called only from generated code during a run.
    let state = unsafe { &mut *state };
    let was_enabled = state.rflags & (1 << 9) != 0;
    state.rflags |= 1 << 9;
    state.sti_shadow = Some(if was_enabled {
        od_isa::StiShadow::Clear
    } else {
        od_isa::StiShadow::Blocking
    });
}

/// Admission of one instruction of generated code in the STI shadow (it clears after it).
pub(crate) fn admits_in_shadow(state: &LongModeState) -> bool {
    const TF: u64 = 1 << 8;
    const RF: u64 = 1 << 16;
    state.rflags & (TF | RF) == 0
        && state.sti_shadow == Some(od_isa::StiShadow::Blocking)
        && state.activity.is_none_or(|activity| activity == od_isa::CpuActivity::Running)
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
const PTE_ACCESSED: u64 = 1 << 5;
const PTE_DIRTY: u64 = 1 << 6;
const PTE_PS: u64 = 1 << 7;
const PTE_NX: u64 = 1 << 63;
const PTE_ADDRESS_RESERVED: u64 = ((1u64 << 16) - 1) << 36;
const PTE_ADDRESS_MASK: u64 = ((1u64 << 36) - 1) & !0xfff;
const PTE_2M_RESERVED: u64 = 0x1fe000;

/// The translation of `linear` for a CPL0 access of `kind` with the checks of the general path's
/// walker, reading the tables through host pointers and writing nothing; `None` for anything the
/// fast form does not cover (a fault, a table outside RAM, an operand outside RAM/framebuffer):
/// the general path's walker decides those. On success [`apply_ad`] makes the accessed and dirty
/// bits that walker sets.
pub(crate) fn fast_translate(
    ram: &PhysicalMemory,
    paging: &Long64ProbeConfig,
    linear: u64,
    kind: WalkKind,
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
        no_execute |= value & PTE_NX != 0;
        leaf = value;
        if is_large {
            large = true;
            break;
        }
        base = value & PTE_ADDRESS_MASK;
    }
    if kind == WalkKind::Write && !writable && paging.write_protect() {
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
            let wanted = if leaf && write { PTE_ACCESSED | PTE_DIRTY } else { PTE_ACCESSED };
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
        let mut value = walk.values[level];
        let leaf = level + 1 == walk.count;
        let wanted = if leaf && write {
            PTE_ACCESSED | PTE_DIRTY
        } else {
            PTE_ACCESSED
        };
        if value & wanted != wanted {
            value |= wanted;
            // SAFETY: the entry pointer of the walk, an aligned u64 in host RAM.
            unsafe {
                std::sync::atomic::AtomicU64::from_ptr(walk.entries[level])
                    .store(value, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}
