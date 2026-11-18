//! S6-36, sensitivity part 1 (SMP.md 5.1): the MP and 2+2W patterns on the RAM access functions of
//! S6-30/S6-32 themselves, by host threads in a tight loop, with NO interpreter between two
//! accesses (an interpreted guest executes hundreds of host instructions between two guest
//! accesses and can hardly catch a hardware reordering, so the guest litmus alone does not show
//! that the barriers matter).
//!
//! Each round is made on a fresh pair of cells (`x` and `y` on cache lines of their own), reset by
//! thread 0 and published by a host spin barrier; both threads then leave the barrier together, wait
//! a pseudo-random number of host spins (so that the windows of the race are swept), and make their
//! accesses through [`PhysicalMemory::load_u32`]/[`PhysicalMemory::store_u32`] of a memory in the
//! parallel mode. A second barrier ends the round.
//!
//! * MP: P0 writes `x = 1` then `y = 1`; P1 reads `y` then `x`. Forbidden: `r(y) = 1, r(x) = 0`.
//! * 2+2W: P0 writes `x = 1` then `y = 1`; P1 writes `y = 2` then `x = 2`. Forbidden final state:
//!   `(x = 1, y = 2)`. `(x = 2, y = 1)` is allowed (`P0:x=1; P1:y=2; P1:x=2; P0:y=1`) and is not a
//!   failure.
//!
//! In the normal build (`SeqCst` ordinary accesses) neither forbidden outcome may appear. With the
//! test-only feature `weak-host-ordering` (ordinary accesses `Relaxed`, no host fence) the same
//! test is run to see whether THIS host shows the weakening; if it does not, the count of rounds
//! is kept as the result (the plan allows it, S6-36 step 4), and the feature is shown to be in
//! force by the recorded order of the accesses ([`the_parallel_accesses_use_the_order_of_the_build`]).

use crate::ram_storage::traced_orders;
use crate::{
    ExecutionMode, GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryRegionKind,
    MemoryRegionSpec, PhysicalMemory, RamLimits,
};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const BASE: u64 = 0x1000;
/// Cells used in turn: `x` at `BASE + slot * STRIDE`, `y` one 4 KiB page above it (its own cache line
/// and its own page generation counter).
const SLOTS: u64 = 64;
const STRIDE: u64 = 0x2000;
const SIZE: u64 = 0x1000 + SLOTS * STRIDE;
/// The largest pseudo-random wait (host spins) after the barrier; `OD_S6_HOST_ORDER_JITTER` changes it.
const JITTER_DEFAULT: u64 = 4;

fn jitter() -> u64 {
    static JITTER: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *JITTER.get_or_init(|| {
        std::env::var("OD_S6_HOST_ORDER_JITTER")
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(JITTER_DEFAULT)
    })
}

fn memory() -> PhysicalMemory {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), SIZE + 0x1000).unwrap();
    let mut memory = PhysicalMemory::new(
        &MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap(),
        RamLimits::new(1 << 24),
    )
    .unwrap();
    memory.set_execution_mode(ExecutionMode::Parallel);
    memory
}

/// A spin barrier of two threads (host atomics only, no RAM access function).
struct Spin {
    count: AtomicU32,
    generation: AtomicU64,
}

impl Spin {
    fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            generation: AtomicU64::new(0),
        }
    }

    fn wait(&self) {
        let generation = self.generation.load(Ordering::SeqCst);
        if self.count.fetch_add(1, Ordering::SeqCst) == 1 {
            self.count.store(0, Ordering::SeqCst);
            self.generation.fetch_add(1, Ordering::SeqCst);
        } else {
            while self.generation.load(Ordering::SeqCst) == generation {
                std::hint::spin_loop();
            }
        }
    }
}

fn mix(round: u64, thread: u64) -> u64 {
    let mut x = round
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((thread + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn at(slot: u64, second: bool) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(BASE + slot * STRIDE + if second { 0x1000 } else { 0 })
}

fn store(memory: &PhysicalMemory, address: GuestPhysicalAddress, value: u32) {
    memory.store_u32(address, value, |_| {}).unwrap();
}

fn load(memory: &PhysicalMemory, address: GuestPhysicalAddress) -> u32 {
    memory.load_u32(address).unwrap()
}

/// The histogram of a pattern: `counts[a][b]` for the two observed values (`0..=2` each).
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct Histogram {
    pub(crate) rounds: u64,
    pub(crate) counts: [[u64; 3]; 3],
}

impl Histogram {
    fn add(&mut self, first: u32, second: u32) {
        self.rounds += 1;
        self.counts[first as usize][second as usize] += 1;
    }

    fn merge(&mut self, other: &Histogram) {
        self.rounds += other.rounds;
        for (row, other_row) in self.counts.iter_mut().zip(&other.counts) {
            for (cell, other_cell) in row.iter_mut().zip(other_row) {
                *cell += other_cell;
            }
        }
    }
}

/// MP: the histogram of `(r(y), r(x))`. Forbidden: `(1, 0)`.
pub(crate) fn run_mp(rounds: u64) -> Histogram {
    let memory = memory();
    let (spin, mut histogram, jitter) = (Spin::new(), Histogram::default(), jitter().max(1));
    std::thread::scope(|scope| {
        let (memory, spin) = (&memory, &spin);
        let writer = scope.spawn(move || {
            for round in 0..rounds {
                let slot = round % SLOTS;
                store(memory, at(slot, false), 0);
                store(memory, at(slot, true), 0);
                spin.wait();
                for _ in 0..mix(round, 0) % jitter {
                    std::hint::spin_loop();
                }
                store(memory, at(slot, false), 1); // x = 1
                store(memory, at(slot, true), 1); // y = 1
                spin.wait();
            }
        });
        let reader = scope.spawn(move || {
            let mut seen = Histogram::default();
            for round in 0..rounds {
                let slot = round % SLOTS;
                spin.wait();
                for _ in 0..mix(round, 1) % jitter {
                    std::hint::spin_loop();
                }
                let y = load(memory, at(slot, true));
                let x = load(memory, at(slot, false));
                spin.wait();
                seen.add(y, x);
            }
            seen
        });
        writer.join().unwrap();
        histogram.merge(&reader.join().unwrap());
    });
    histogram
}

/// 2+2W: the histogram of the final `(x, y)`. Forbidden: `(1, 2)`.
pub(crate) fn run_2plus2w(rounds: u64) -> Histogram {
    let memory = memory();
    let (spin, mut histogram, jitter) = (Spin::new(), Histogram::default(), jitter().max(1));
    std::thread::scope(|scope| {
        let (memory, spin) = (&memory, &spin);
        let p0 = scope.spawn(move || {
            let mut seen = Histogram::default();
            for round in 0..rounds {
                let slot = round % SLOTS;
                store(memory, at(slot, false), 0);
                store(memory, at(slot, true), 0);
                spin.wait();
                for _ in 0..mix(round, 0) % jitter {
                    std::hint::spin_loop();
                }
                store(memory, at(slot, false), 1); // x = 1
                store(memory, at(slot, true), 1); // y = 1
                spin.wait();
                // Both threads are done: the final state.
                seen.add(load(memory, at(slot, false)), load(memory, at(slot, true)));
            }
            seen
        });
        let p1 = scope.spawn(move || {
            for round in 0..rounds {
                let slot = round % SLOTS;
                spin.wait();
                for _ in 0..mix(round, 1) % jitter {
                    std::hint::spin_loop();
                }
                store(memory, at(slot, true), 2); // y = 2
                store(memory, at(slot, false), 2); // x = 2
                spin.wait();
            }
        });
        p1.join().unwrap();
        histogram.merge(&p0.join().unwrap());
    });
    histogram
}

fn rounds_from_env(default: u64) -> u64 {
    std::env::var("OD_S6_HOST_ORDER_ROUNDS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(default)
}

fn report(rounds: u64) -> (Histogram, Histogram) {
    let started = std::time::Instant::now();
    let (mp, w) = (run_mp(rounds), run_2plus2w(rounds));
    eprintln!(
        "host order litmus [{}]: {rounds} rounds each, {:.1} s\n  MP    (r(y), r(x)) counts {:?}; forbidden (1,0): {}\n  2+2W  final (x, y) counts {:?}; forbidden (1,2): {}",
        if cfg!(feature = "weak-host-ordering") {
            "weak-host-ordering"
        } else {
            "normal build"
        },
        started.elapsed().as_secs_f64(),
        mp.counts,
        mp.counts[1][0],
        w.counts,
        w.counts[1][2],
    );
    (mp, w)
}

fn assert_sane(mp: &Histogram, w: &Histogram, rounds: u64) {
    // Every round counted once; MP read only 0/1, 2+2W ended with only 1/2.
    assert_eq!((mp.rounds, w.rounds), (rounds, rounds));
    assert_eq!(mp.counts[2].iter().sum::<u64>(), 0);
    assert_eq!(mp.counts.iter().map(|row| row[2]).sum::<u64>(), 0);
    assert_eq!(w.counts[0].iter().sum::<u64>(), 0);
    assert_eq!(w.counts.iter().map(|row| row[0]).sum::<u64>(), 0);
}

/// The normal build: with the barriers, neither forbidden outcome appears.
#[test]
#[cfg(not(feature = "weak-host-ordering"))]
fn with_the_barriers_no_forbidden_outcome_appears_on_the_ram_access_functions() {
    let rounds = rounds_from_env(200_000);
    let (mp, w) = report(rounds);
    assert_sane(&mp, &w, rounds);
    assert_eq!(
        mp.counts[1][0], 0,
        "MP: r(y) = 1, r(x) = 0 appeared: {mp:?}"
    );
    assert_eq!(w.counts[1][2], 0, "2+2W: (x = 1, y = 2) appeared: {w:?}");
}

/// The long form of the normal build: `OD_S6_HOST_ORDER_ROUNDS` rounds each (default 20 million).
#[test]
#[cfg(not(feature = "weak-host-ordering"))]
#[ignore = "long: OD_S6_HOST_ORDER_ROUNDS=20000000 cargo test --release -p od-machine --offline --lib -- --ignored host_order_litmus::with_the_barriers_long --nocapture"]
fn with_the_barriers_long_no_forbidden_outcome_appears() {
    let rounds = rounds_from_env(20_000_000);
    let (mp, w) = report(rounds);
    assert_sane(&mp, &w, rounds);
    assert_eq!(mp.counts[1][0], 0, "MP: {mp:?}");
    assert_eq!(w.counts[1][2], 0, "2+2W: {w:?}");
}

/// The weakened build (`--features weak-host-ordering`): the same patterns, no assertion about
/// whether the forbidden outcome shows (that depends on the host: the count is the result and is
/// recorded); only that the run is consistent. The feature itself is checked below.
#[test]
#[cfg(feature = "weak-host-ordering")]
#[ignore = "sensitivity run: OD_S6_HOST_ORDER_ROUNDS=20000000 cargo test --release -p od-machine --offline --lib --features weak-host-ordering -- --ignored host_order_litmus --nocapture"]
fn the_weakened_build_reports_what_this_host_shows() {
    let rounds = rounds_from_env(20_000_000);
    let (mp, w) = report(rounds);
    assert_sane(&mp, &w, rounds);
}

/// The parallel-mode scalar accesses are `SeqCst` in the normal build and `Relaxed` in the build
/// with `weak-host-ordering`: the weakening is really in force whatever the host shows. A guest
/// fence places a host fence in the normal build and none in the weakened one.
#[test]
fn the_parallel_accesses_use_the_order_of_the_build() {
    let memory = memory();
    let address = at(0, false);
    let (_, orders) = traced_orders(|| {
        store(&memory, address, 7);
        load(&memory, address)
    });
    let expected = if cfg!(feature = "weak-host-ordering") {
        Ordering::Relaxed
    } else {
        Ordering::SeqCst
    };
    assert_eq!(orders, vec![expected, expected]);
    let before = memory.host_fences_issued();
    memory.fence(od_isa::FenceKind::Full);
    let issued = memory.host_fences_issued() - before;
    assert_eq!(issued, u64::from(!cfg!(feature = "weak-host-ordering")));
}
