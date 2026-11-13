//! Deterministic fault backend with a volatile/durable byte model.
//!
//! # Model
//!
//! [`FaultHandle`] owns the state of one backing object:
//!
//! * `volatile` - what reads see right after a write completes;
//! * `durable` - what is on media after the last successful durable flush;
//! * `pending` - the ordered mutations (writes, resizes) accepted since the
//!   last durable barrier. They are the only thing a crash may still decide.
//!
//! [`FaultBackend`] is a [`StorageBackend`] view of that state. A crash is
//! [`FaultHandle::crash`]: it bumps the epoch (every older backend instance is
//! dead and fails with an I/O error, like a handle of a killed process), turns
//! `pending` into one outcome chosen by a [`CrashPolicy`], and sets both
//! `durable` and `volatile` to it. [`FaultHandle::open`] then yields a fresh
//! backend over the post-crash bytes (a reopen).
//!
//! # Failure model
//!
//! Allowed by this backend:
//!
//! * a crash loses any subset of the writes accepted since the last durable
//!   barrier;
//! * those writes may persist in any order relative to each other, but never
//!   across a barrier: a successful flush (or an ambiguous flush that made
//!   data durable) empties `pending`;
//! * a write persists in pieces split at absolute 512-byte sector
//!   boundaries; each piece is kept or lost independently (torn write). A
//!   sector piece itself is atomic;
//! * a resize is kept or lost as a unit; once a resize is lost every later
//!   pending mutation is lost too (resizes are not reordered with anything);
//! * scripted per-operation faults (see [`Fault`]): failed, short and torn
//!   writes, failed resize, failed flush that did or did not reach media.
//!
//! Not allowed (a backend that does this is broken, not "unlucky"):
//!
//! * losing, changing or reordering bytes already covered by a flush that
//!   returned success;
//! * changing durable bytes without an operation;
//! * data that was never written appearing after a crash.
//!
//! [`FlushFault::OkWithoutDurability`] deliberately violates the first rule.
//! It exists only so tests can prove that an independent oracle notices a
//! backend that acknowledges without flushing.
//!
//! Namespace durability (rename, directory entry) is not modelled; the backend
//! declares `namespace_durability == false`.
//!
//! # Determinism and diagnostics
//!
//! Faults are scripted by operation ordinal: the zero-based index over every
//! mutating backend call (`write_at`, `set_len`, `flush_file_durable`). A
//! single `CheckedStorage::write_all_at` may use several ordinals when the
//! backend writes short. Crash outcomes depend only on the policy seed. Every
//! mutating call, crash and reopen is appended to a trace; failures should be
//! reported through [`FaultHandle::report`], which includes the run seed and
//! the whole trace.

use crate::io::{BackendError, StorageBackend, StorageCapabilities};
use crate::testing::memory::MEMORY_MAX_LEN;
use crate::testing::rng::SplitMix64;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::rc::Rc;

/// Size of the unit that a crash keeps or loses as a whole.
pub const SECTOR: usize = 512;

/// Fault for a `write_at` call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteFault {
    /// Nothing is written; an I/O error is returned.
    Fail(ErrorKind),
    /// Only the first `n` bytes are written; `Ok(n)` is returned (a normal
    /// short transfer).
    Short(usize),
    /// The first `written` bytes reach the volatile view, then an I/O error is
    /// returned. The caller learns nothing about how much was written.
    TornIo { written: usize },
}

/// Fault for a `flush_file_durable` call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushFault {
    /// The backend explicitly refuses file-data durability before performing
    /// the flush; pending bytes and acknowledgement state are unchanged.
    Unsupported,
    /// I/O error; nothing became durable (`pending` is unchanged).
    IoNotDurable,
    /// Lost acknowledgement: everything became durable, then an I/O error is
    /// returned. Indistinguishable for the caller from `IoNotDurable`.
    IoAfterDurable,
    /// Returns success without making anything durable. Outside the failure
    /// model; a deliberately broken backend for oracle detection tests.
    OkWithoutDurability,
}

/// One scripted fault, attached to a mutating-operation ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    Write(WriteFault),
    /// `set_len` fails without changing the size.
    Resize(ErrorKind),
    Flush(FlushFault),
}

/// What a crash does with the writes that are not behind a durable barrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrashPolicy {
    /// Lose every pending mutation (volatile falls back to durable).
    DropPending,
    /// Keep every pending mutation in order (as if nothing was lost).
    KeepPending,
    /// Keep a pseudo-random subset of sector pieces in pseudo-random order.
    Seeded(u64),
}

#[derive(Clone, Debug)]
enum Pending {
    Write { offset: u64, data: Vec<u8> },
    Resize(u64),
}

#[derive(Debug)]
struct State {
    volatile: Vec<u8>,
    durable: Vec<u8>,
    pending: Vec<Pending>,
    script: BTreeMap<u64, Fault>,
    next_ordinal: u64,
    epoch: u64,
    seed: u64,
    trace: Vec<String>,
}

/// Shared control and inspection handle for one fault-injected object.
#[derive(Clone, Debug)]
pub struct FaultHandle {
    state: Rc<RefCell<State>>,
}

impl FaultHandle {
    /// A new object whose volatile and durable bytes are both `initial`.
    /// `seed` is only recorded for [`report`](Self::report).
    pub fn new(initial: &[u8], seed: u64) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                volatile: initial.to_vec(),
                durable: initial.to_vec(),
                pending: Vec::new(),
                script: BTreeMap::new(),
                next_ordinal: 0,
                epoch: 0,
                seed,
                trace: Vec::new(),
            })),
        }
    }

    /// A backend for the current epoch.
    pub fn open(&self) -> FaultBackend {
        let epoch = self.state.borrow().epoch;
        self.log(format!("open epoch={epoch}"));
        FaultBackend {
            state: Rc::clone(&self.state),
            epoch,
        }
    }

    /// Ordinal the next mutating backend call will get.
    pub fn next_ordinal(&self) -> u64 {
        self.state.borrow().next_ordinal
    }

    /// Attaches `fault` to the mutating call with this ordinal.
    pub fn script(&self, ordinal: u64, fault: Fault) {
        let mut st = self.state.borrow_mut();
        if ordinal < st.next_ordinal {
            let msg = format!("fault scripted for ordinal {ordinal} that already happened");
            panic!("{}", format_report(&st, &msg));
        }
        st.script.insert(ordinal, fault);
    }

    /// Attaches `fault` to the very next mutating call.
    pub fn script_next(&self, fault: Fault) {
        self.script(self.next_ordinal(), fault);
    }

    pub fn volatile_bytes(&self) -> Vec<u8> {
        self.state.borrow().volatile.clone()
    }

    pub fn durable_bytes(&self) -> Vec<u8> {
        self.state.borrow().durable.clone()
    }

    /// Number of mutations that a crash could still lose or reorder.
    pub fn pending_len(&self) -> usize {
        self.state.borrow().pending.len()
    }

    /// Simulates power loss or process death. All backends opened before this
    /// call become dead. Returns the post-crash bytes.
    pub fn crash(&self, policy: CrashPolicy) -> Vec<u8> {
        let mut st = self.state.borrow_mut();
        let outcome = crash_outcome(&st.durable, &st.pending, policy);
        let line = format!(
            "crash policy={policy:?} pending={} -> len={}",
            st.pending.len(),
            outcome.len()
        );
        st.trace.push(line);
        st.durable = outcome.clone();
        st.volatile = outcome.clone();
        st.pending.clear();
        st.epoch += 1;
        outcome
    }

    /// Appends a free-form line (harness steps) to the operation trace.
    pub fn log(&self, line: impl Into<String>) {
        self.state.borrow_mut().trace.push(line.into());
    }

    pub fn trace(&self) -> Vec<String> {
        self.state.borrow().trace.clone()
    }

    /// Failure text with the seed and the full operation trace. Use it as the
    /// panic message of every test that drives this handle.
    pub fn report(&self, message: &str) -> String {
        format_report(&self.state.borrow(), message)
    }
}

fn format_report(st: &State, message: &str) -> String {
    let mut out = format!(
        "{message}\nseed={:#018x}\ntrace ({} entries):",
        st.seed,
        st.trace.len()
    );
    for line in &st.trace {
        out.push_str("\n  ");
        out.push_str(line);
    }
    out
}

/// [`StorageBackend`] over a [`FaultHandle`]'s state. Declares file
/// durability, resize and the exclusive lock (so a RAW image can be built on
/// it); namespace durability is not declared.
#[derive(Debug)]
pub struct FaultBackend {
    state: Rc<RefCell<State>>,
    epoch: u64,
}

fn dead_error() -> BackendError {
    BackendError::Io {
        kind: ErrorKind::BrokenPipe,
        os_error: None,
    }
}

fn io_error(kind: ErrorKind) -> BackendError {
    BackendError::Io {
        kind,
        os_error: None,
    }
}

impl FaultBackend {
    fn alive(&self, st: &State) -> Result<(), BackendError> {
        if st.epoch == self.epoch {
            Ok(())
        } else {
            Err(dead_error())
        }
    }

    /// Takes the next ordinal and its scripted fault, if any.
    fn begin(st: &mut State) -> (u64, Option<Fault>) {
        let ordinal = st.next_ordinal;
        st.next_ordinal += 1;
        (ordinal, st.script.remove(&ordinal))
    }
}

fn apply_write(bytes: &mut Vec<u8>, offset: u64, data: &[u8]) {
    let start = offset as usize;
    let end = start + data.len();
    if bytes.len() < end {
        bytes.resize(end, 0);
    }
    bytes[start..end].copy_from_slice(data);
}

/// Splits a write at absolute sector boundaries.
fn sector_pieces(offset: u64, data: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let mut pieces = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let abs = offset + pos as u64;
        let to_boundary = SECTOR - (abs as usize % SECTOR);
        let n = to_boundary.min(data.len() - pos);
        pieces.push((abs, data[pos..pos + n].to_vec()));
        pos += n;
    }
    pieces
}

fn crash_outcome(durable: &[u8], pending: &[Pending], policy: CrashPolicy) -> Vec<u8> {
    let mut out = durable.to_vec();
    match policy {
        CrashPolicy::DropPending => out,
        CrashPolicy::KeepPending => {
            for p in pending {
                match p {
                    Pending::Write { offset, data } => apply_write(&mut out, *offset, data),
                    Pending::Resize(len) => out.resize(*len as usize, 0),
                }
            }
            out
        }
        CrashPolicy::Seeded(seed) => {
            let mut rng = SplitMix64(seed);
            let mut segment: Vec<(u64, Vec<u8>)> = Vec::new();
            for p in pending {
                match p {
                    Pending::Write { offset, data } => {
                        segment.extend(sector_pieces(*offset, data));
                    }
                    Pending::Resize(len) => {
                        flush_segment(&mut out, &mut segment, &mut rng);
                        if rng.below(2) == 0 {
                            out.resize(*len as usize, 0);
                        } else {
                            return out;
                        }
                    }
                }
            }
            flush_segment(&mut out, &mut segment, &mut rng);
            out
        }
    }
}

/// Keeps a random subset of the pieces and applies it in random order.
fn flush_segment(out: &mut Vec<u8>, segment: &mut Vec<(u64, Vec<u8>)>, rng: &mut SplitMix64) {
    let mut kept: Vec<(u64, Vec<u8>)> = segment.drain(..).filter(|_| rng.below(2) == 0).collect();
    for i in (1..kept.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        kept.swap(i, j);
    }
    for (offset, data) in kept {
        apply_write(out, offset, &data);
    }
}

impl StorageBackend for FaultBackend {
    fn capabilities(&self) -> StorageCapabilities {
        let mut caps = StorageCapabilities::read_write(MEMORY_MAX_LEN).with_exclusive_lock();
        caps.file_durability = true;
        caps
    }

    fn size(&self) -> Result<u64, BackendError> {
        let st = self.state.borrow();
        self.alive(&st)?;
        Ok(st.volatile.len() as u64)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        let st = self.state.borrow();
        self.alive(&st)?;
        let start = match usize::try_from(offset) {
            Ok(s) if s < st.volatile.len() => s,
            _ => return Ok(0),
        };
        let n = buf.len().min(st.volatile.len() - start);
        buf[..n].copy_from_slice(&st.volatile[start..start + n]);
        Ok(n)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        let mut st = self.state.borrow_mut();
        self.alive(&st)?;
        let (ordinal, fault) = Self::begin(&mut st);
        let (n, result) = match fault {
            None => (buf.len(), Ok(buf.len())),
            Some(Fault::Write(WriteFault::Fail(kind))) => (0, Err(io_error(kind))),
            Some(Fault::Write(WriteFault::Short(n))) => {
                let n = n.min(buf.len());
                (n, Ok(n))
            }
            Some(Fault::Write(WriteFault::TornIo { written })) => {
                (written.min(buf.len()), Err(io_error(ErrorKind::Other)))
            }
            Some(other) => {
                let msg = format!("#{ordinal} write_at got fault {other:?} meant for another op");
                st.trace.push(msg.clone());
                panic!("{}", format_report(&st, &msg));
            }
        };
        let end = offset + n as u64;
        if end > MEMORY_MAX_LEN {
            st.trace.push(format!(
                "#{ordinal} write off={offset} len={} -> StorageFull",
                buf.len()
            ));
            return Err(io_error(ErrorKind::StorageFull));
        }
        if n > 0 {
            let data = buf[..n].to_vec();
            apply_write(&mut st.volatile, offset, &data);
            st.pending.push(Pending::Write { offset, data });
        }
        st.trace.push(format!(
            "#{ordinal} write off={offset} len={} fault={fault:?} accepted={n} -> {result:?}",
            buf.len()
        ));
        result
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        let mut st = self.state.borrow_mut();
        self.alive(&st)?;
        let (ordinal, fault) = Self::begin(&mut st);
        let result = match fault {
            None => Ok(()),
            Some(Fault::Resize(kind)) => Err(io_error(kind)),
            Some(other) => {
                let msg = format!("#{ordinal} set_len got fault {other:?} meant for another op");
                st.trace.push(msg.clone());
                panic!("{}", format_report(&st, &msg));
            }
        };
        if result.is_ok() {
            st.volatile.resize(len as usize, 0);
            st.pending.push(Pending::Resize(len));
        }
        st.trace.push(format!(
            "#{ordinal} set_len len={len} fault={fault:?} -> {result:?}"
        ));
        result
    }

    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        let mut st = self.state.borrow_mut();
        self.alive(&st)?;
        let (ordinal, fault) = Self::begin(&mut st);
        let (make_durable, result) = match fault {
            None => (true, Ok(())),
            Some(Fault::Flush(FlushFault::Unsupported)) => (
                false,
                Err(BackendError::DurabilityUnsupported {
                    scope: crate::io::DurabilityScope::File,
                }),
            ),
            Some(Fault::Flush(FlushFault::IoNotDurable)) => {
                (false, Err(io_error(ErrorKind::Other)))
            }
            Some(Fault::Flush(FlushFault::IoAfterDurable)) => {
                (true, Err(io_error(ErrorKind::Other)))
            }
            Some(Fault::Flush(FlushFault::OkWithoutDurability)) => (false, Ok(())),
            Some(other) => {
                let msg = format!("#{ordinal} flush got fault {other:?} meant for another op");
                st.trace.push(msg.clone());
                panic!("{}", format_report(&st, &msg));
            }
        };
        if make_durable {
            st.durable = st.volatile.clone();
            st.pending.clear();
        }
        st.trace.push(format!(
            "#{ordinal} flush fault={fault:?} durable_now={make_durable} -> {result:?}"
        ));
        result
    }
}
