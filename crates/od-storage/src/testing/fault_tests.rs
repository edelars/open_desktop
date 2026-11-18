//! RAW write -> flush -> crash -> reopen against [`FaultBackend`], checked by
//! an independent oracle.
//!
//! # What is guaranteed (and what is not)
//!
//! After a `DurableAck` for the file scope, a crash and a reopen:
//!
//! * every byte equals the oracle's *acknowledged snapshot* (the bytes as
//!   they were when the ack was issued) **unless** a later, unacknowledged
//!   write covered that byte. A lost acknowledged write is a violation;
//! * a byte covered by later unacknowledged writes is either the acknowledged
//!   byte or a byte written by one of those writes. Which one is *not*
//!   guaranteed: unacknowledged writes may vanish, persist partially (torn at
//!   512-byte sectors) and persist in any order;
//! * after a flush that returned an error the outcome is *unknown*: it is
//!   either the old durable state or the new one (or, for a crash, anything in
//!   the unacknowledged set). The caller can not tell which, and rollback to
//!   the old state is not promised.
//!
//! The oracle below is a plain `Vec<u8>` model updated with slice copies. It
//! uses neither `FaultBackend` internals nor `RawImage`/`CheckedStorage`, and
//! no parser, so no shared bug can hide a mismatch.

use crate::io::{CheckedStorage, DurabilityScope, IoOp, StorageError};
use crate::raw::RawImage;
use crate::testing::fault::{
    CrashPolicy, Fault, FaultBackend, FaultHandle, FlushFault, SECTOR, WriteFault,
};
use crate::testing::rng::SplitMix64;
use std::collections::BTreeSet;
use std::io::ErrorKind;

const IMAGE: usize = 4 * SECTOR;
const BLOCK: u64 = SECTOR as u64;

/// Independent logical model of the object.
struct Oracle {
    /// Bytes as the writer sees them (every successful write applied).
    current: Vec<u8>,
    /// State known to be on media: the initial bytes, the bytes at the last
    /// acknowledged flush, or what a previous crash left behind.
    base: Vec<u8>,
    /// Writes since `base` was established, in submission order.
    pending: Vec<(usize, Vec<u8>)>,
    /// Number of writes accepted (the sequence).
    write_seq: u64,
    /// `write_seq` at the last acknowledgement (the durable watermark).
    watermark: u64,
    acks: u64,
    /// Every pending write reached the backend as one whole call (no short,
    /// torn or failed-and-retried transfer), so sectors are atomic units.
    whole: bool,
}

impl Oracle {
    fn new(initial: &[u8]) -> Self {
        Self {
            current: initial.to_vec(),
            base: initial.to_vec(),
            pending: Vec::new(),
            write_seq: 0,
            watermark: 0,
            acks: 0,
            whole: true,
        }
    }

    fn write(&mut self, offset: usize, data: &[u8]) {
        self.current[offset..offset + data.len()].copy_from_slice(data);
        self.pending.push((offset, data.to_vec()));
        self.write_seq += 1;
    }

    /// Like [`write`](Self::write) for a write that went through a fault
    /// (short, torn, failed and retried): its pieces may persist separately
    /// even inside one sector, so sector-atomicity is no longer claimed.
    fn write_faulted(&mut self, offset: usize, data: &[u8]) {
        self.write(offset, data);
        self.whole = false;
    }

    /// A durable acknowledgement was given: everything so far is durable.
    fn ack(&mut self) {
        self.base = self.current.clone();
        self.pending.clear();
        self.watermark = self.write_seq;
        self.acks += 1;
        self.whole = true;
    }

    /// Does `observed` belong to the set of outcomes the failure model allows
    /// for a crash right now? Per byte: the base byte or a byte of a pending
    /// write covering it.
    fn check(&self, observed: &[u8]) -> Result<(), String> {
        if observed.len() != self.base.len() {
            return Err(format!(
                "length {} != expected {}",
                observed.len(),
                self.base.len()
            ));
        }
        for (i, &got) in observed.iter().enumerate() {
            if got == self.base[i] {
                continue;
            }
            let explained = self
                .pending
                .iter()
                .any(|(off, data)| i >= *off && i < off + data.len() && data[i - off] == got);
            if !explained {
                return Err(format!(
                    "byte {i}: observed {got:#04x}, acknowledged/base {:#04x}, \
                     {} pending writes cannot explain it (watermark={}, seq={})",
                    self.base[i],
                    self.pending.len(),
                    self.watermark,
                    self.write_seq
                ));
            }
        }
        Ok(())
    }

    /// Stricter check used while every pending write is a whole backend call
    /// (see `whole`): a sector touched by exactly one pending write is either
    /// entirely base or entirely base-plus-that-write (sector atomicity).
    /// Falls back to the per-byte check otherwise.
    fn check_sector_atomic(&self, observed: &[u8]) -> Result<(), String> {
        self.check(observed)?;
        if !self.whole {
            return Ok(());
        }
        for s in 0..self.base.len() / SECTOR {
            let (lo, hi) = (s * SECTOR, (s + 1) * SECTOR);
            let touching: Vec<_> = self
                .pending
                .iter()
                .filter(|(off, data)| *off < hi && off + data.len() > lo)
                .collect();
            if touching.len() != 1 {
                continue;
            }
            let (off, data) = touching[0];
            let mut with_write = self.base[lo..hi].to_vec();
            let (from, to) = (lo.max(*off), hi.min(off + data.len()));
            with_write[from - lo..to - lo].copy_from_slice(&data[from - off..to - off]);
            let got = &observed[lo..hi];
            if got != &self.base[lo..hi] && got != with_write.as_slice() {
                return Err(format!("sector {s} is neither old nor new"));
            }
        }
        Ok(())
    }

    /// A crash happened and `observed` was read back: it is now the media
    /// state. Validates it first.
    fn adopt_crash(&mut self, observed: &[u8]) -> Result<(), String> {
        self.check_sector_atomic(observed)?;
        self.base = observed.to_vec();
        self.current = observed.to_vec();
        self.pending.clear();
        self.whole = true;
        Ok(())
    }
}

// ---------------------------------------------------------------- helpers

/// `assert!` whose panic message carries the seed and operation trace.
macro_rules! ensure {
    ($handle:expr, $cond:expr, $($arg:tt)+) => {
        if !$cond {
            panic!("{}", $handle.report(&format!($($arg)+)));
        }
    };
}

/// `assert_eq!` with seed and trace; values are printed truncated.
macro_rules! ensure_eq {
    ($handle:expr, $left:expr, $right:expr, $($arg:tt)+) => {{
        let (l, r) = (&$left, &$right);
        if l != r {
            panic!(
                "{}",
                $handle.report(&format!(
                    "{}: left={} right={}",
                    format!($($arg)+),
                    short(l),
                    short(r)
                ))
            );
        }
    }};
}

fn short<T: std::fmt::Debug + ?Sized>(value: &T) -> String {
    let text = format!("{value:?}");
    match text.char_indices().nth(120) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text,
    }
}

fn open_image(handle: &FaultHandle, size: usize) -> RawImage<FaultBackend> {
    RawImage::open_read_write(handle.open(), Some(size as u64), BLOCK).expect("open RAW image")
}

fn new_image(handle: &FaultHandle, size: usize) -> RawImage<FaultBackend> {
    RawImage::create(handle.open(), size as u64, BLOCK).expect("create RAW image")
}

fn read_image(image: &RawImage<FaultBackend>) -> Vec<u8> {
    let mut buf = vec![0u8; image.logical_size() as usize];
    image.read_at(0, &mut buf).expect("read image");
    buf
}

/// Durable flush through the RAW image. On success the image is rebuilt over
/// the same backend; on failure the (sticky) storage is returned.
fn flush_image(
    image: RawImage<FaultBackend>,
) -> Result<RawImage<FaultBackend>, CheckedStorage<FaultBackend>> {
    let size = image.logical_size();
    let mut storage = image.into_storage();
    match storage.flush_file_durable() {
        Ok(ack) => {
            assert_eq!(ack.scope(), DurabilityScope::File);
            let backend = storage.into_backend().ok().expect("no reconcile after ack");
            Ok(RawImage::open_read_write(backend, Some(size), BLOCK).expect("rewrap"))
        }
        Err(_) => Err(storage),
    }
}

/// Crashes, reopens and reads back. The bytes `crash()` reports and the bytes
/// a reopened RAW image reads must be identical, so the oracle check that the
/// caller runs on the result covers both.
fn crash_and_read(handle: &FaultHandle, policy: CrashPolicy) -> Vec<u8> {
    let direct = handle.crash(policy);
    let read = read_image(&open_image(handle, handle.volatile_bytes().len()));
    ensure!(
        handle,
        direct == read,
        "crash() bytes differ from reopened read ({policy:?})"
    );
    read
}

fn fail(handle: &FaultHandle, msg: &str) -> ! {
    panic!("{}", handle.report(msg))
}

// ------------------------------------------------------------------- tests

/// (a) The first RAW case: exactly the announced guarantee.
#[test]
fn raw_write_flush_crash_reopen_guarantee() {
    let policies = || {
        let mut v = vec![CrashPolicy::DropPending, CrashPolicy::KeepPending];
        v.extend((0..64).map(CrashPolicy::Seeded));
        v
    };
    for policy in policies() {
        let handle = FaultHandle::new(&[0u8; IMAGE], 0xA);
        let mut rng = SplitMix64(0xA);
        let mut oracle = Oracle::new(&[0u8; IMAGE]);
        let mut image = new_image(&handle, IMAGE);

        let a = rng.bytes(700);
        image.write_at(100, &a).expect("write A");
        oracle.write(100, &a);
        handle.log("harness: write A off=100 len=700");
        // Not durable yet: the backend ground truth is still the old image.
        ensure_eq!(
            handle,
            handle.durable_bytes(),
            vec![0u8; IMAGE],
            "write must not be durable before flush"
        );

        let image = flush_image(image).unwrap_or_else(|_| fail(&handle, "flush A failed"));
        oracle.ack();
        handle.log("harness: ack after flush A");
        ensure_eq!(
            handle,
            handle.durable_bytes(),
            oracle.base,
            "ack means durable"
        );
        ensure_eq!(handle, handle.pending_len(), 0, "barrier empties pending");

        let mut image = image;
        let b = rng.bytes(600);
        image.write_at(300, &b).expect("write B");
        oracle.write(300, &b);
        handle.log("harness: write B off=300 len=600 (unacknowledged)");
        drop(image);

        let observed = crash_and_read(&handle, policy);
        oracle
            .check(&observed)
            .unwrap_or_else(|m| fail(&handle, &m));
        // Exactly the announced guarantee: bytes outside B's range are the
        // acknowledged ones, whatever the crash did.
        ensure_eq!(
            handle,
            observed[..300],
            oracle.base[..300],
            "acked prefix {policy:?}"
        );
        ensure_eq!(
            handle,
            observed[900..],
            oracle.base[900..],
            "acked suffix {policy:?}"
        );
        match policy {
            CrashPolicy::DropPending => {
                ensure_eq!(handle, observed, oracle.base, "drop-all = acked snapshot")
            }
            CrashPolicy::KeepPending => {
                ensure_eq!(handle, observed, oracle.current, "keep-all = writer view")
            }
            CrashPolicy::Seeded(_) => {}
        }
    }
}

/// (b) Deliberately broken orderings must be caught by the oracle.
#[test]
fn oracle_detects_ack_without_durability() {
    // Broken backend: flush reports success but makes nothing durable.
    let handle = FaultHandle::new(&[0u8; IMAGE], 0xB1);
    let mut oracle = Oracle::new(&[0u8; IMAGE]);
    let mut image = new_image(&handle, IMAGE);
    let data = SplitMix64(1).bytes(300);
    image.write_at(0, &data).expect("write");
    oracle.write(0, &data);
    handle.script_next(Fault::Flush(FlushFault::OkWithoutDurability));
    let image = flush_image(image).ok().expect("lying flush acks");
    drop(image);
    oracle.ack();
    let observed = crash_and_read(&handle, CrashPolicy::DropPending);
    let err = oracle.check(&observed).expect_err("lost acked write");
    ensure!(
        handle,
        err.contains("byte 0"),
        "unexpected detection: {err}"
    );

    // Broken writer: acknowledges to its caller without calling flush.
    let handle = FaultHandle::new(&[0u8; IMAGE], 0xB2);
    let mut oracle = Oracle::new(&[0u8; IMAGE]);
    let mut image = new_image(&handle, IMAGE);
    image.write_at(512, &data).expect("write");
    oracle.write(512, &data);
    oracle.ack(); // no flush_image
    drop(image);
    let observed = crash_and_read(&handle, CrashPolicy::DropPending);
    ensure!(
        handle,
        oracle.check(&observed).is_err(),
        "early ack without flush went undetected"
    );

    // Control: the honest order passes under the same crash.
    let handle = FaultHandle::new(&[0u8; IMAGE], 0xB3);
    let mut oracle = Oracle::new(&[0u8; IMAGE]);
    let mut image = new_image(&handle, IMAGE);
    image.write_at(512, &data).expect("write");
    oracle.write(512, &data);
    let image = flush_image(image).ok().expect("flush");
    oracle.ack();
    drop(image);
    let observed = crash_and_read(&handle, CrashPolicy::DropPending);
    ensure_eq!(handle, oracle.check(&observed), Ok(()), "honest control");
}

/// (c) A failed flush is sticky; a crash does not reset that.
#[test]
fn flush_error_is_sticky_and_not_an_ack() {
    for fault in [FlushFault::IoNotDurable, FlushFault::IoAfterDurable] {
        let handle = FaultHandle::new(&[0u8; IMAGE], 0xC);
        let mut oracle = Oracle::new(&[0u8; IMAGE]);
        let mut image = new_image(&handle, IMAGE);
        let data = SplitMix64(3).bytes(512);
        image.write_at(0, &data).expect("write");
        oracle.write(0, &data);

        handle.script_next(Fault::Flush(fault));
        let mut storage = flush_image(image).err().expect("flush must fail");
        // No ack was produced: the oracle watermark does not move.
        ensure_eq!(
            handle,
            oracle.acks,
            0,
            "no ack for a failed flush ({fault:?})"
        );
        ensure_eq!(
            handle,
            storage.flush_file_durable(),
            Err(StorageError::ReconcileRequired),
            "second flush ({fault:?})"
        );
        ensure_eq!(
            handle,
            storage.write_all_at(0, &[1]),
            Err(StorageError::ReconcileRequired),
            "write after failed flush ({fault:?})"
        );
        ensure_eq!(
            handle,
            storage.set_len(IMAGE as u64),
            Err(StorageError::ReconcileRequired),
            "resize after failed flush ({fault:?})"
        );
        let storage = storage.into_backend().expect_err("backend not released");

        let observed = {
            handle.crash(CrashPolicy::DropPending);
            // The old storage is dead (epoch) and still sticky; it never
            // yields an ack, even after the crash.
            let mut storage = storage;
            ensure_eq!(
                handle,
                storage.flush_file_durable(),
                Err(StorageError::ReconcileRequired),
                "flush after crash ({fault:?})"
            );
            drop(storage);
            read_image(&open_image(&handle, IMAGE))
        };
        // The outcome is old or new, and the oracle has no ack for it.
        oracle
            .check(&observed)
            .unwrap_or_else(|m| fail(&handle, &m));
        ensure!(
            handle,
            observed == oracle.base || observed == oracle.current,
            "outcome is neither old nor new ({fault:?})"
        );
        ensure_eq!(handle, oracle.acks, 0, "still no ack ({fault:?})");
    }
}

/// (d) Torn writes: not guaranteed before flush, fully durable after it.
#[test]
fn torn_write_before_flush_is_not_guaranteed_but_flushed_is() {
    let old = SplitMix64(7).bytes(2 * SECTOR);
    let new = SplitMix64(8).bytes(500);
    let (off, end) = (300usize, 800usize); // spans sector 0 and sector 1

    let mut outcomes = BTreeSet::new();
    for seed in 0..64u64 {
        let handle = FaultHandle::new(&old, seed);
        let mut oracle = Oracle::new(&old);
        let mut image = RawImage::create(handle.open(), old.len() as u64, BLOCK).expect("create");
        image.write_at(off as u64, &new).expect("write");
        oracle.write(off, &new);
        drop(image);
        let observed = crash_and_read(&handle, CrashPolicy::Seeded(seed));
        oracle
            .check_sector_atomic(&observed)
            .unwrap_or_else(|m| fail(&handle, &m));
        let sector0_new = observed[off..SECTOR] == new[..SECTOR - off];
        let sector1_new = observed[SECTOR..end] == new[SECTOR - off..];
        outcomes.insert((sector0_new, sector1_new));
    }
    // Before a flush all four combinations are legal, including a torn one
    // and a complete loss; the model really produces them.
    {
        let handle = FaultHandle::new(&old, 0xD0);
        ensure!(
            handle,
            outcomes.contains(&(false, false)),
            "complete loss never produced: {outcomes:?}"
        );
        ensure!(
            handle,
            outcomes.contains(&(true, false)) || outcomes.contains(&(false, true)),
            "no torn outcome produced: {outcomes:?}"
        );
    }

    // API-level torn write: some bytes reach the volatile view, then Io.
    let handle = FaultHandle::new(&old, 0xD);
    let mut image = RawImage::create(handle.open(), old.len() as u64, BLOCK).expect("create");
    handle.script_next(Fault::Write(WriteFault::TornIo { written: 100 }));
    ensure!(
        handle,
        image.write_at(off as u64, &new).is_err(),
        "torn write must report an error"
    );
    let mut expect = old.clone();
    expect[off..off + 100].copy_from_slice(&new[..100]);
    ensure_eq!(
        handle,
        handle.volatile_bytes(),
        expect,
        "visible prefix of torn write"
    );
    ensure_eq!(handle, handle.durable_bytes(), old, "nothing durable yet");
    // Retry, then flush: after the ack, the whole write is durable under
    // every crash outcome.
    image.write_at(off as u64, &new).expect("retry");
    drop(flush_image(image).unwrap_or_else(|_| fail(&handle, "flush")));
    let mut full = old.clone();
    full[off..end].copy_from_slice(&new);
    for policy in [
        CrashPolicy::DropPending,
        CrashPolicy::KeepPending,
        CrashPolicy::Seeded(1),
        CrashPolicy::Seeded(2),
    ] {
        let observed = crash_and_read(&handle, policy);
        ensure_eq!(
            handle,
            observed,
            full,
            "flushed write must survive {policy:?}"
        );
    }
}

/// (e) Lost ack: the caller sees the same error whether or not the data
/// reached media, so the outcome is only known to be old or new.
#[test]
fn lost_ack_outcome_is_old_or_new_and_indistinguishable() {
    let old = SplitMix64(11).bytes(IMAGE);
    let data = SplitMix64(12).bytes(SECTOR);
    let mut new = old.clone();
    new[SECTOR..2 * SECTOR].copy_from_slice(&data);

    let mut errors = Vec::new();
    let mut outcomes = Vec::new();
    let mut last_handle = None;
    for fault in [FlushFault::IoNotDurable, FlushFault::IoAfterDurable] {
        let handle = FaultHandle::new(&old, 0xE);
        let mut image = RawImage::create(handle.open(), IMAGE as u64, BLOCK).expect("create");
        image.write_at(SECTOR as u64, &data).expect("write");
        handle.script_next(Fault::Flush(fault));
        let mut storage = image.into_storage();
        errors.push(storage.flush_file_durable().expect_err("flush fails"));
        drop(storage);
        outcomes.push(crash_and_read(&handle, CrashPolicy::DropPending));
        last_handle = Some(handle);
    }
    let handle = last_handle.expect("ran");
    // Same visible error: the caller cannot tell the two cases apart ...
    ensure_eq!(handle, errors[0], errors[1], "caller-visible flush errors");
    ensure!(
        handle,
        matches!(
            errors[0],
            StorageError::Io {
                op: IoOp::FlushFile,
                ..
            }
        ),
        "unexpected flush error {:?}",
        errors[0]
    );
    // ... yet the media differ. Both are inside {old, new}; rollback is not
    // something the failed call promises.
    ensure_eq!(handle, outcomes[0], old, "IoNotDurable leaves old");
    ensure_eq!(handle, outcomes[1], new, "IoAfterDurable leaves new");
    ensure!(handle, outcomes[0] != outcomes[1], "outcomes must differ");
}

#[test]
fn resize_error_and_resize_loss() {
    let handle = FaultHandle::new(&[1u8; SECTOR], 0xF);
    let mut storage = CheckedStorage::new(handle.open()).expect("caps");
    handle.script_next(Fault::Resize(ErrorKind::StorageFull));
    let result = storage.set_len(2 * SECTOR as u64);
    ensure!(
        handle,
        matches!(
            result,
            Err(StorageError::Io {
                op: IoOp::Resize,
                kind: ErrorKind::StorageFull,
                ..
            })
        ),
        "unexpected resize result {result:?}"
    );
    ensure_eq!(
        handle,
        storage.size(),
        Ok(SECTOR as u64),
        "size after error"
    );
    storage.set_len(2 * SECTOR as u64).expect("grow");
    ensure_eq!(
        handle,
        storage.size(),
        Ok(2 * SECTOR as u64),
        "size after grow"
    );
    drop(storage);
    // An unflushed resize may be lost: size falls back to the durable one.
    ensure_eq!(
        handle,
        handle.crash(CrashPolicy::DropPending).len(),
        SECTOR,
        "lost resize"
    );
}

// ---------------------------------------------------------- seeded runs

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Break {
    None,
    /// Backend flushes lie (success without durability).
    LyingFlush,
    /// Writer acknowledges early, without flushing.
    EarlyAck,
}

#[derive(Clone, Copy)]
struct RunConfig {
    breakage: Break,
    /// Also compare backend durable bytes with the oracle at every ack.
    check_watermark: bool,
}

const MASTER_SEED: u64 = 0x0DE5_C0DE_5EED_0007;
const SEQUENCES: u64 = 200;
const STEPS: usize = 24;

/// One random sequence. `Err` carries the full report (seed + trace).
fn run_sequence(seed: u64, cfg: RunConfig) -> Result<(), String> {
    let mut rng = SplitMix64(seed);
    let handle = FaultHandle::new(&[0u8; IMAGE], seed);
    let mut oracle = Oracle::new(&[0u8; IMAGE]);
    let mut image = Some(new_image(&handle, IMAGE));

    macro_rules! bad {
        ($($arg:tt)*) => {
            return Err(handle.report(&format!($($arg)*)))
        };
    }

    for step in 0..STEPS {
        let mut img = image.take().expect("image");
        match rng.below(10) {
            0..=5 => {
                let offset = rng.below(IMAGE as u64 - 1) as usize;
                let len = 1 + rng.below((IMAGE - offset).min(900) as u64) as usize;
                let data = rng.bytes(len);
                let mut faulted = false;
                if rng.below(100) < 20 {
                    faulted = true;
                    let fault = match rng.below(3) {
                        0 => WriteFault::Fail(ErrorKind::Other),
                        1 => WriteFault::Short(1 + rng.below(len as u64) as usize),
                        _ => WriteFault::TornIo {
                            written: rng.below(len as u64 + 1) as usize,
                        },
                    };
                    handle.script_next(Fault::Write(fault));
                }
                handle.log(format!(
                    "harness: step {step} write off={offset} len={len} faulted={faulted}"
                ));
                if img.write_at(offset as u64, &data).is_err() {
                    // A failed write may be partial; the writer retries the
                    // whole range, so the intended state is exact.
                    if let Err(e) = img.write_at(offset as u64, &data) {
                        bad!("step {step}: retry of failed write failed: {e:?}");
                    }
                }
                if faulted {
                    oracle.write_faulted(offset, &data);
                } else {
                    oracle.write(offset, &data);
                }
                if read_image(&img) != oracle.current {
                    bad!("step {step}: visible bytes differ from oracle");
                }
                image = Some(img);
            }
            6..=7 => {
                // Draw everything this branch can need up front, so the
                // random stream (writes, crash points) is identical for the
                // honest and the broken configurations of the same seed.
                let fault_roll = rng.below(100);
                let fault_kind = rng.below(2);
                let restart_policy = random_policy(&mut rng);
                if cfg.breakage == Break::EarlyAck {
                    handle.log(format!("harness: step {step} EARLY ACK (no flush)"));
                    oracle.ack();
                    image = Some(img);
                    continue;
                }
                let fault = match cfg.breakage {
                    Break::LyingFlush => Some(FlushFault::OkWithoutDurability),
                    _ if fault_roll < 25 => Some(if fault_kind == 0 {
                        FlushFault::IoNotDurable
                    } else {
                        FlushFault::IoAfterDurable
                    }),
                    _ => None,
                };
                if let Some(f) = fault {
                    handle.script_next(Fault::Flush(f));
                }
                handle.log(format!("harness: step {step} flush fault={fault:?}"));
                match flush_image(img) {
                    Ok(img) => {
                        oracle.ack();
                        if cfg.check_watermark && handle.durable_bytes() != oracle.base {
                            bad!("step {step}: ack given but durable bytes != oracle snapshot");
                        }
                        image = Some(img);
                    }
                    Err(storage) => {
                        // Outcome unknown, no ack: pending stays pending and
                        // the only way forward is a restart.
                        drop(storage);
                        let observed = crash_and_read(&handle, restart_policy);
                        if let Err(m) = oracle.adopt_crash(&observed) {
                            bad!("step {step}: after failed flush: {m}");
                        }
                        image = Some(open_image(&handle, IMAGE));
                    }
                }
            }
            _ => {
                drop(img);
                let policy = random_policy(&mut rng);
                handle.log(format!("harness: step {step} crash+reopen {policy:?}"));
                let observed = crash_and_read(&handle, policy);
                if let Err(m) = oracle.adopt_crash(&observed) {
                    bad!("step {step}: after crash: {m}");
                }
                image = Some(open_image(&handle, IMAGE));
            }
        }
    }
    // Final crash so that every sequence ends on a checked crash outcome.
    drop(image);
    let observed = crash_and_read(&handle, random_policy(&mut rng));
    if let Err(m) = oracle.adopt_crash(&observed) {
        bad!("final crash: {m}");
    }
    Ok(())
}

fn random_policy(rng: &mut SplitMix64) -> CrashPolicy {
    match rng.below(4) {
        0 => CrashPolicy::DropPending,
        1 => CrashPolicy::KeepPending,
        _ => CrashPolicy::Seeded(rng.next_u64()),
    }
}

/// (f) 200 seeded sequences, every crash checked by the oracle.
#[test]
fn seeded_runs_match_oracle() {
    let mut master = SplitMix64(MASTER_SEED);
    for i in 0..SEQUENCES {
        let seed = master.next_u64();
        run_sequence(
            seed,
            RunConfig {
                breakage: Break::None,
                check_watermark: true,
            },
        )
        .unwrap_or_else(|report| panic!("sequence {i} failed:\n{report}"));
    }
}

/// (b, seeded) Broken flush ordering/ack is found by the seeded harness.
/// Every failure of a broken configuration must be the specific detection
/// that configuration is supposed to trigger; any other error (a harness or
/// backend bug) fails the test. Reports carry the seed and the trace.
#[test]
fn seeded_runs_detect_broken_flush_and_ack() {
    for (cfg, marker) in [
        // Lost acked bytes are seen at the next crash.
        (
            RunConfig {
                breakage: Break::LyingFlush,
                check_watermark: false,
            },
            "cannot explain",
        ),
        // The watermark check fires right at the ack.
        (
            RunConfig {
                breakage: Break::LyingFlush,
                check_watermark: true,
            },
            "ack given but durable",
        ),
        (
            RunConfig {
                breakage: Break::EarlyAck,
                check_watermark: false,
            },
            "cannot explain",
        ),
    ] {
        let mut master = SplitMix64(MASTER_SEED);
        let mut detected = 0;
        for _ in 0..SEQUENCES {
            let seed = master.next_u64();
            let Err(report) = run_sequence(seed, cfg) else {
                continue;
            };
            detected += 1;
            assert!(
                report.contains(marker),
                "{:?}: unexpected failure kind, wanted {marker:?}:\n{report}",
                cfg.breakage
            );
            assert!(report.contains("seed=0x"), "{report}");
            assert!(report.contains("trace ("), "{report}");
            assert!(report.contains("harness: step"), "{report}");
            assert!(
                report.contains("after crash")
                    || report.contains("final crash")
                    || report.contains("after failed flush")
                    || report.contains("ack given"),
                "{report}"
            );
        }
        assert!(
            detected > 0,
            "{:?} was not detected in {SEQUENCES} runs",
            cfg.breakage
        );
    }
}

#[test]
fn run_is_deterministic_for_a_seed() {
    let cfg = RunConfig {
        breakage: Break::LyingFlush,
        check_watermark: false,
    };
    let a = (1..100u64).find_map(|s| run_sequence(s, cfg).err());
    let b = (1..100u64).find_map(|s| run_sequence(s, cfg).err());
    assert!(a.is_some());
    assert_eq!(a, b);
}

/// S9-38: RAW write + durable flush with every fault of every kind at every
/// mutating call × every crash policy. RAW promises no atomicity of a
/// multi-sector write (STORAGE_FORMAT §4.1), so the checks are what it does
/// promise: a failing fault is never acknowledged, an acknowledged write
/// survives every crash, and bytes outside the written range never change.
#[test]
fn raw_write_flush_full_fault_matrix() {
    use crate::cow_proto_tests::{OpKind, faults_for, is_failing, parse_kind, policies};
    let mut rng = SplitMix64(0x3808);
    let old = rng.bytes(IMAGE);
    let at = SECTOR / 2;
    let data = rng.bytes(2 * SECTOR + 7);
    let mut new = old.clone();
    new[at..at + data.len()].copy_from_slice(&data);
    let prepare = |seed: u64| {
        // The old bytes are durable before the measured operation.
        let handle = FaultHandle::new(&old, seed);
        let image = open_image(&handle, IMAGE);
        (handle, image)
    };
    let (clean, image) = prepare(0x3809);
    let start = clean.next_ordinal();
    let mut image = image;
    image.write_at(at as u64, &data).expect("clean write");
    assert!(flush_image(image).is_ok());
    let kinds: Vec<OpKind> = clean
        .trace()
        .iter()
        .filter_map(|l| parse_kind(l))
        .filter(|(o, _)| *o >= start)
        .map(|(_, k)| k)
        .collect();
    assert!(kinds.len() >= 2, "{kinds:?}");
    let (mut cases, mut acked_n) = (0u64, 0u64);
    for (i, kind) in kinds.iter().enumerate() {
        for fault in faults_for(*kind) {
            for policy in policies() {
                cases += 1;
                let (handle, mut image) = prepare(0x3809);
                handle.script(handle.next_ordinal() + i as u64, fault);
                let acked = image.write_at(at as u64, &data).is_ok()
                    && match flush_image(image) {
                        Ok(_) => true,
                        Err(_) => false,
                    };
                let label = format!("op={i} {fault:?} {policy:?}");
                if is_failing(fault) {
                    ensure!(handle, !acked, "{label}: failing fault acknowledged");
                }
                let after = handle.crash(policy);
                ensure!(handle, after.len() == IMAGE, "{label}: size changed");
                if acked {
                    acked_n += 1;
                    ensure!(handle, after == new, "{label}: acknowledged write lost");
                }
                ensure!(
                    handle,
                    after[..at] == old[..at] && after[at + data.len()..] == old[at + data.len()..],
                    "{label}: bytes outside the write changed"
                );
            }
        }
    }
    // Power cut after the last call of an acknowledged write.
    for policy in policies() {
        cases += 1;
        let (handle, mut image) = prepare(0x3809);
        image.write_at(at as u64, &data).expect("write");
        assert!(flush_image(image).is_ok());
        ensure!(
            handle,
            handle.crash(policy) == new,
            "clean {policy:?}: lost"
        );
        acked_n += 1;
    }
    assert!(cases >= 20 && acked_n > 0, "{cases} {acked_n}");
}
