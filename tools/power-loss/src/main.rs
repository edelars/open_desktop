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
//! Exit codes: 0 pass or run finished; 1 usage; 2 verification failed; 3 I/O
//! error while committing.

use od_host::storage_io::HostFileStorage;
use od_storage::cow_proto::{BLOCK_SIZE, CowFile, CowView, crc32};
use od_storage::io::CheckedStorage;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    if ack.starts_with(&dir) {
        return Err((1, "the ack log must be outside the test directory".into()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| io_err("create dir", e))?;
    let image = dir.join("cow.img");
    let writer = HostFileStorage::create_new_exclusive(&image, 0)
        .map_err(|e| io_err("create image", e))?;
    let storage = CheckedStorage::new(writer).map_err(|e| io_err("storage", e))?;
    let (mut cow, _) = CowFile::create(storage).map_err(|e| io_err("create cow", e))?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ack)
        .map_err(|e| io_err("open ack log", e))?;
    let mut note = |line: &str| -> Result<(), (u8, String)> {
        writeln!(log, "{line}").map_err(|e| io_err("write ack log", e))?;
        log.sync_all().map_err(|e| io_err("sync ack log", e))
    };
    note(&format!(
        "start seed={seed} commits={commits} generation={}",
        cow.generation()
    ))?;
    for c in 1..=commits {
        let writes = plan(seed, c);
        let slices: Vec<(u64, &[u8])> = writes.iter().map(|(b, d)| (*b, d.as_slice())).collect();
        match cow.commit(&slices) {
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
    let mut problems: Vec<String> = Vec::new();
    for line in log.lines() {
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
            if acked_gen > 0 {
                problems.push(format!("acknowledged data but image refused: {e:?}"));
            }
            return finish(&image, &ack, acked_commit, acked_gen, None, problems);
        }
    };
    let durable_gen = view.generation();
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
    finish(&image, &ack, acked_commit, acked_gen, Some(durable_gen), problems)
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
            "usage: od-power-loss run --dir D --ack LOG --seed S --commits N [--pause-ms P]\n       od-power-loss verify --image I --ack LOG --seed S".to_string(),
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
