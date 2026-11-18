//! S9-39 driver for the CoW v1 prototype (STORAGE_FORMAT §9) on a removable
//! device.
//!
//! `run` creates `<dir>/cow.img` and commits deterministic transactions. After
//! each `DurableAck` it appends `commit=C generation=G digest=D` to an
//! acknowledgement log, fsynced. The log must live outside `<dir>`, on a device
//! that is not being cut. The operator removes power from the test device while
//! `run` is active; the next commit fails and the error is recorded.
//!
//! `verify` reopens the image read-only, checks the durable generation against
//! the log, and replays every commit to compare each block byte for byte.
//!
//! Cut points (`--cut`, for commit `--cut-commit`): `block0` and `rootcopy`
//! are before barrier 1 (the commit can still vanish whole); `after-b1` is
//! after barrier 1, before the root write; `before-b2` is after the root
//! write, before barrier 2; `after-b2` is after barrier 2 returned, before the
//! acknowledgement is logged — the driver first logs `flushed commit=C
//! generation=G`, and `verify` then requires that generation to be durable:
//! this is the point that tests the barrier itself; `idle` is after the
//! acknowledgement. Barrier-side points are taken by a backend wrapper around
//! the host file (`Cutter`), not by the storage library.
//!
//! Exit codes: 0 pass or run finished; 1 usage; 2 verification failed; 3 I/O
//! error while committing.

use od_host::storage_io::HostFileStorage;
use od_storage::cow_proto::{BLOCK_SIZE, CommitStep, CowFile, CowView, crc32};
use od_storage::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// Logical size bound of the test image, in blocks.
const MAX_BLOCKS: u64 = 32;
/// Generation of the empty state `create` leaves (commit 1 is generation 3).
const CREATE_BASE: u64 = 2;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn word(seed: u64, commit: u64, salt: u64) -> u64 {
    mix(seed ^ mix(commit.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt))
}

/// Block contents for commit `c` at block `b`, deterministic from `seed`.
fn block(seed: u64, c: u64, b: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(BLOCK_SIZE);
    let mut state = word(seed, c, b.wrapping_add(0x1000));
    out.extend_from_slice(&c.to_le_bytes());
    out.extend_from_slice(&b.to_le_bytes());
    while out.len() < BLOCK_SIZE {
        state = mix(state.wrapping_add(0x9E37_79B9_7F4A_7C15));
        out.extend_from_slice(&state.to_le_bytes());
    }
    out
}

/// Writes of commit `c` (1-based): distinct blocks, never a gap in the image.
fn plan(seed: u64, c: u64) -> Vec<(u64, Vec<u8>)> {
    let new_len = c.min(MAX_BLOCKS);
    let prev_len = (c - 1).min(MAX_BLOCKS);
    let mut picks: BTreeMap<u64, ()> = BTreeMap::new();
    if new_len > prev_len {
        picks.insert(new_len - 1, ());
    }
    for j in 0..(1 + c % 3) {
        picks.insert(word(seed, c, j) % new_len, ());
    }
    picks.keys().map(|&b| (b, block(seed, c, b))).collect()
}

fn flatten(writes: &[(u64, Vec<u8>)]) -> Vec<u8> {
    writes.iter().flat_map(|(_, d)| d.iter().copied()).collect()
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn need(args: &[String], name: &str) -> Result<String, (u8, String)> {
    arg(args, name).ok_or((1, format!("missing {name}")))
}

/// Where the operator removes power: `idle` after the acknowledgement of the
/// cut commit, `block0` before its first data block, `rootcopy` before its root
/// copy (the last point at which the commit can still be abandoned).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CutStage {
    Idle,
    Block0,
    RootCopy,
    AfterB1,
    BeforeB2,
    AfterB2,
}

struct Cut {
    stage: CutStage,
    commit: u64,
    window: Duration,
}

fn parse_cut(args: &[String]) -> Result<Option<Cut>, (u8, String)> {
    let Some(stage) = arg(args, "--cut") else {
        return Ok(None);
    };
    let stage = match stage.as_str() {
        "idle" => CutStage::Idle,
        "block0" => CutStage::Block0,
        "rootcopy" => CutStage::RootCopy,
        "after-b1" => CutStage::AfterB1,
        "before-b2" => CutStage::BeforeB2,
        "after-b2" => CutStage::AfterB2,
        _ => {
            return Err((
                1,
                "--cut must be idle, block0, rootcopy, after-b1, before-b2 or after-b2".into(),
            ));
        }
    };
    let commit = need(args, "--cut-commit")?
        .parse()
        .map_err(|_| (1, "bad --cut-commit".to_string()))?;
    let secs: u64 = arg(args, "--window-s")
        .map(|v| v.parse().map_err(|_| (1, "bad --window-s".to_string())))
        .transpose()?
        .unwrap_or(120);
    Ok(Some(Cut {
        stage,
        commit,
        window: Duration::from_secs(secs),
    }))
}

/// The acknowledgement log, appended and fsynced line by line.
type Log = Rc<RefCell<std::fs::File>>;

fn note(log: &Log, line: &str) -> Result<(), (u8, String)> {
    let mut f = log.borrow_mut();
    writeln!(f, "{line}").map_err(|e| io_err("write ack log", e))?;
    f.sync_all().map_err(|e| io_err("sync ack log", e))
}

/// Pauses for the operator's window and records whether the device is still
/// present when the window ends (`present=false` means the cut happened).
fn cut_window(log: &Log, cut: &Cut, dir: &Path) -> Result<(), (u8, String)> {
    let note = |line: &str| note(log, line);
    note(&format!(
        "cutpoint stage={:?} commit={} window_s={}",
        cut.stage,
        cut.commit,
        cut.window.as_secs()
    ))?;
    // Poll so that a device removed and reattached inside the window is still
    // seen as absent.
    // `absent_after_ms` is the delay from the cut point to the first poll
    // (every 50 ms) that saw the device gone: the time the drive had to
    // drain its own cache after the last barrier.
    let start = std::time::Instant::now();
    let deadline = start + cut.window;
    let mut absent_after_ms: Option<u128> = None;
    while std::time::Instant::now() < deadline {
        if absent_after_ms.is_none() && !dir.exists() {
            absent_after_ms = Some(start.elapsed().as_millis());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let present = dir.exists();
    note(&format!(
        "window_end absent_seen={} absent_after_ms={} present={present}",
        absent_after_ms.is_some(),
        absent_after_ms.map_or_else(|| "none".to_string(), |ms| ms.to_string())
    ))
}

/// Host file wrapper that opens the cut window around the barriers of the cut
/// commit. `CowFile::commit` makes exactly two durable flushes (barrier 1,
/// barrier 2); `armed` is set only around the cut commit.
struct Cutter {
    inner: HostFileStorage,
    state: Rc<RefCell<CutState>>,
}

struct CutState {
    armed: bool,
    flushes: u32,
    fired: bool,
    /// First failure to log inside a window; `run` stops with it.
    error: Option<(u8, String)>,
    cut: Option<Cut>,
    log: Log,
    dir: PathBuf,
}

impl CutState {
    fn window_if(&mut self, stage: CutStage, before_line: Option<String>) {
        let Some(cut) = self.cut.as_ref().filter(|c| c.stage == stage) else {
            return;
        };
        if !self.armed || self.fired {
            return;
        }
        self.fired = true;
        let mut logged = before_line.map_or(Ok(()), |line| note(&self.log, &line));
        if logged.is_ok() {
            logged = cut_window(&self.log, cut, &self.dir);
        }
        if let Err(e) = logged {
            self.error.get_or_insert(e);
        }
    }
}

impl StorageBackend for Cutter {
    fn capabilities(&self) -> StorageCapabilities {
        self.inner.capabilities()
    }
    fn size(&self) -> Result<u64, BackendError> {
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.inner.read_at(offset, buf)
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        self.inner.write_at(offset, buf)
    }
    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.inner.set_len(len)
    }
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        let n = {
            let mut st = self.state.borrow_mut();
            if st.armed {
                st.flushes += 1;
            }
            st.flushes
        };
        if n == 2 {
            self.state.borrow_mut().window_if(CutStage::BeforeB2, None);
        }
        self.inner.flush_file_durable()?;
        let mut st = self.state.borrow_mut();
        match n {
            1 => st.window_if(CutStage::AfterB1, None),
            2 => {
                let line = st.cut.as_ref().map(|c| {
                    format!(
                        "flushed commit={} generation={}",
                        c.commit,
                        c.commit + CREATE_BASE
                    )
                });
                st.window_if(CutStage::AfterB2, line);
            }
            _ => {}
        }
        Ok(())
    }
}

fn io_err(context: &str, e: impl std::fmt::Debug) -> (u8, String) {
    (3, format!("{context}: {e:?}"))
}

fn run(args: &[String]) -> Result<(), (u8, String)> {
    let dir = PathBuf::from(need(args, "--dir")?);
    let ack = PathBuf::from(need(args, "--ack")?);
    let seed: u64 = need(args, "--seed")?
        .parse()
        .map_err(|_| (1, "bad --seed".to_string()))?;
    let commits: u64 = need(args, "--commits")?
        .parse()
        .map_err(|_| (1, "bad --commits".to_string()))?;
    let pause_ms: u64 = arg(args, "--pause-ms")
        .map(|v| v.parse().map_err(|_| (1, "bad --pause-ms".to_string())))
        .transpose()?
        .unwrap_or(0);
    let cut = parse_cut(args)?;
    if let Some(k) = cut.as_ref() {
        if k.commit == 0 || k.commit > commits {
            return Err((1, "--cut-commit must be in 1..=--commits".into()));
        }
    }
    if ack.starts_with(&dir) {
        return Err((1, "the ack log must be outside the test directory".into()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| io_err("create dir", e))?;
    let image = dir.join("cow.img");
    let writer =
        HostFileStorage::create_new_exclusive(&image, 0).map_err(|e| io_err("create image", e))?;
    let log: Log = Rc::new(RefCell::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ack)
            .map_err(|e| io_err("open ack log", e))?,
    ));
    let cut_commit = cut.as_ref().map(|k| k.commit);
    let barrier_cut = cut.as_ref().is_some_and(|k| {
        matches!(
            k.stage,
            CutStage::AfterB1 | CutStage::BeforeB2 | CutStage::AfterB2
        )
    });
    let state = Rc::new(RefCell::new(CutState {
        armed: false,
        flushes: 0,
        fired: false,
        error: None,
        cut: if barrier_cut {
            cut.as_ref().map(|k| Cut {
                stage: k.stage,
                commit: k.commit,
                window: k.window,
            })
        } else {
            None
        },
        log: Rc::clone(&log),
        dir: dir.clone(),
    }));
    let storage = CheckedStorage::new(Cutter {
        inner: writer,
        state: Rc::clone(&state),
    })
    .map_err(|e| io_err("storage", e))?;
    let (mut cow, _) = CowFile::create(storage).map_err(|e| io_err("create cow", e))?;
    let note = |line: &str| note(&log, line);
    note(&format!(
        "start seed={seed} commits={commits} generation={}",
        cow.generation()
    ))?;
    for c in 1..=commits {
        let writes = plan(seed, c);
        let slices: Vec<(u64, &[u8])> = writes.iter().map(|(b, d)| (*b, d.as_slice())).collect();
        let cut_here = cut.as_ref().filter(|k| k.commit == c);
        let mut fired = false;
        let mut hook_error: Option<(u8, String)> = None;
        {
            let mut st = state.borrow_mut();
            st.armed = cut_commit == Some(c);
            st.flushes = 0;
        }
        let result = {
            let mut step_hook = |step: CommitStep| -> bool {
                let Some(k) = cut_here else { return false };
                let hit = matches!(
                    (k.stage, step),
                    (CutStage::Block0, CommitStep::Block(0))
                        | (CutStage::RootCopy, CommitStep::RootCopy)
                );
                if hit && !fired {
                    fired = true;
                    if let Err(e) = cut_window(&log, k, &dir) {
                        hook_error.get_or_insert(e);
                    }
                }
                false
            };
            cow.commit_cancellable(&slices, &mut step_hook)
        };
        match result {
            Ok(_) => {
                let digest = crc32(&flatten(&writes));
                note(&format!(
                    "commit={c} generation={} digest={digest:08x}",
                    cow.generation()
                ))?;
            }
            Err(e) => {
                let _ = note(&format!("error commit={c} detail={e:?}"));
                return Err((3, format!("commit {c} failed: {e:?}")));
            }
        }
        state.borrow_mut().armed = false;
        if let Some(e) = hook_error.take() {
            return Err(e);
        }
        if let Some(e) = state.borrow_mut().error.take() {
            return Err(e);
        }
        if let Some(k) = cut_here.filter(|k| k.stage == CutStage::Idle) {
            cut_window(&log, k, &dir)?;
        }
        if pause_ms > 0 {
            std::thread::sleep(Duration::from_millis(pause_ms));
        }
    }
    note("done")?;
    Ok(())
}

fn verify(args: &[String]) -> Result<bool, (u8, String)> {
    let image = PathBuf::from(need(args, "--image")?);
    let ack = PathBuf::from(need(args, "--ack")?);
    let seed: u64 = need(args, "--seed")?
        .parse()
        .map_err(|_| (1, "bad --seed".to_string()))?;
    let log = std::fs::read_to_string(&ack).map_err(|e| io_err("read ack log", e))?;
    let mut acked_commit = 0u64;
    let mut acked_gen = 0u64;
    // A commit whose barrier 2 returned before the cut: its generation must
    // be durable although the acknowledgement was never logged.
    let mut flushed_gen = 0u64;
    // Cut before the root write of commit C (block0, rootcopy, after-b1):
    // that commit can never be published.
    let mut unpublishable: Option<u64> = None;
    let mut problems: Vec<String> = Vec::new();
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("cutpoint stage=") {
            let mut parts = rest.split_whitespace();
            let stage = parts.next().unwrap_or("");
            let c: Option<u64> = parts
                .next()
                .and_then(|v| v.strip_prefix("commit="))
                .and_then(|v| v.parse().ok());
            if matches!(stage, "Block0" | "RootCopy" | "AfterB1") {
                unpublishable = c;
            }
        }
        if let Some(rest) = line.strip_prefix("flushed commit=") {
            flushed_gen = rest
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.strip_prefix("generation="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        }
        if let Some(rest) = line.strip_prefix("commit=") {
            let mut parts = rest.split_whitespace();
            let c: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let g: u64 = parts
                .next()
                .and_then(|v| v.strip_prefix("generation="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if g != c + CREATE_BASE {
                problems.push(format!("log: commit {c} has generation {g}"));
            }
            acked_commit = acked_commit.max(c);
            acked_gen = acked_gen.max(g);
        }
    }
    let backend = HostFileStorage::open_read_only(&image).map_err(|e| io_err("open image", e))?;
    let storage = CheckedStorage::new(backend).map_err(|e| io_err("storage", e))?;
    let view = match CowView::inspect(storage) {
        Ok((view, _)) => view,
        Err(e) => {
            if acked_gen > 0 || flushed_gen > 0 {
                problems.push(format!("acknowledged data but image refused: {e:?}"));
            }
            return finish(&image, &ack, acked_commit, acked_gen, None, problems);
        }
    };
    let durable_gen = view.generation();
    if let Some(c) = unpublishable.filter(|c| *c > acked_commit) {
        if durable_gen > acked_gen {
            problems.push(format!(
                "commit {c} was cut before its root write but generation {durable_gen} is durable (acknowledged {acked_gen})"
            ));
        }
    }
    if durable_gen < flushed_gen {
        problems.push(format!(
            "generation {flushed_gen} was flushed by barrier 2 before the cut but is lost: durable {durable_gen}"
        ));
    }
    if durable_gen < acked_gen {
        problems.push(format!(
            "acknowledged generation {acked_gen} lost: durable {durable_gen}"
        ));
    }
    if durable_gen > acked_gen + 1 {
        problems.push(format!(
            "durable generation {durable_gen} exceeds acknowledged {acked_gen} by more than one"
        ));
    }
    let durable_commit = durable_gen.saturating_sub(CREATE_BASE);
    let mut model: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for c in 1..=durable_commit {
        for (b, d) in plan(seed, c) {
            model.insert(b, d);
        }
    }
    let expected_len = durable_commit.min(MAX_BLOCKS);
    if view.block_count() != expected_len {
        problems.push(format!(
            "block count {} but expected {expected_len}",
            view.block_count()
        ));
    }
    for (b, d) in &model {
        match view.read_block(*b) {
            Ok(got) if got == *d => {}
            Ok(_) => problems.push(format!("block {b} differs from the replayed commits")),
            Err(e) => problems.push(format!("block {b}: {e:?}")),
        }
    }
    finish(
        &image,
        &ack,
        acked_commit,
        acked_gen,
        Some(durable_gen),
        problems,
    )
}

fn finish(
    image: &Path,
    ack: &Path,
    acked_commit: u64,
    acked_gen: u64,
    durable_gen: Option<u64>,
    problems: Vec<String>,
) -> Result<bool, (u8, String)> {
    let ok = problems.is_empty();
    let line = format!(
        "verify ok={ok} image={} ack={} acked_commit={acked_commit} acked_generation={acked_gen} durable_generation={} problems={}",
        image.display(),
        ack.display(),
        durable_gen.map_or_else(|| "none".to_string(), |g| g.to_string()),
        problems.len()
    );
    println!("{line}");
    for p in &problems {
        println!("problem: {p}");
    }
    Ok(ok)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]).map(|()| true),
        Some("verify") => verify(&args[1..]),
        _ => Err((
            1,
            "usage: od-power-loss run --dir D --ack LOG --seed S --commits N [--pause-ms P] [--cut idle|block0|rootcopy|after-b1|before-b2|after-b2 --cut-commit C [--window-s W]]\n       od-power-loss verify --image I --ack LOG --seed S".to_string(),
        )),
    };
    match result {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(2),
        Err((code, message)) => {
            eprintln!("{message}");
            std::process::exit(i32::from(code));
        }
    }
}
