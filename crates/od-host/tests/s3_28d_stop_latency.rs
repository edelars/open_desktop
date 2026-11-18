//! S3-28d: LOCAL wall-time measurement of the cooperative STOP latency on a busy
//! JIT path. Local only: the machine that runs it, one vCPU-equivalent worker
//! thread, no Mini, no promise of any number of milliseconds.
//!
//! The test is `#[ignore]`d and not part of the normal suite. Run:
//!
//! ```text
//! cargo test -p od-host --release --offline --test s3_28d_stop_latency \
//!     -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Environment: `OD_S3_28D_RUNS` (default 1000) and `OD_S3_28D_OUT` (default
//! `docs/reports/data/stage3/s3-28d`, relative to the repository root).
//!
//! Protocol, per run: a worker thread runs the cached register dispatcher over a
//! guest loop that never ends by itself (budget 2^62). The control thread waits
//! until the worker has executed at least 1000 more blocks than at the start
//! of the run, sleeps a pseudo-random 0..200 microseconds so the STOP does not
//! align with the loop phase, takes `t0 = Instant::now()`, and stores
//! `stop_requested = 1` (Release). The worker takes `t1 = Instant::now()` right
//! after the dispatcher returns `StopRequested`. Latency = `t1 - t0`. It
//! includes the store, the observation by a generated guard or by the
//! dispatcher boundary, the unwinding of the native frame, result decoding and
//! one `Instant::now()` call; it excludes thread wake-up (the worker is busy).

use std::io::Write as _;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use od_host::{PublishedBlockExecutionError, PublishedBlockExecutor, PublishedCode};
use od_isa::LongModeState;
use od_jit::block::{CodeView, NativeRegisterV1Admission};
use od_jit::block_compile::BlockCompileError;
use od_jit::dispatch::{
    BlockExecutionReport, CachedBlockBackend, DispatchControl, DispatchError, DispatchExit,
    SingleEntryBlockCache, run_cached_native_register_v1_controlled,
};
use od_jit::runtime::RuntimeExitTag;
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};

/// Counts started native block executions so the control thread knows the
/// worker is busy; otherwise a plain pass-through to the production backend.
struct CountingBackend<'a> {
    inner: PublishedBlockExecutor,
    blocks: &'a AtomicU64,
}

impl CachedBlockBackend for CountingBackend<'_> {
    type Error = PublishedBlockExecutionError;
    type Published = PublishedCode;

    fn publish_cached_block(
        &mut self,
        block: &od_jit::block_compile::CompiledBlock,
    ) -> Result<Self::Published, Self::Error> {
        self.inner.publish_cached_block(block)
    }

    fn execute_cached_block(
        &mut self,
        published: &Self::Published,
        block: &od_jit::block_compile::CompiledBlock,
        state: &mut LongModeState,
        incoming_budget: u64,
        control: DispatchControl<'_>,
    ) -> Result<BlockExecutionReport, Self::Error> {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        self.inner
            .execute_cached_block(published, block, state, incoming_budget, control)
    }
}

struct Config {
    name: &'static str,
    description: String,
    code: Vec<u8>,
}

/// `n` x NOT RAX (flag-preserving) ; JZ entry. The initial RFLAGS has ZF set, so
/// the loop never ends. One block of `n + 1` instructions.
fn not_block(n: usize) -> Vec<u8> {
    let mut code = Vec::new();
    for _ in 0..n {
        code.extend_from_slice(&[0x48, 0xF7, 0xD0]);
    }
    code.extend_from_slice(&[0x74, (-(3 * n as i64 + 2)) as i8 as u8]);
    code
}

/// The compile pipeline caps emitted code at 4096 bytes
/// (`EmittedCodeTooLarge`), which is below the 32-instruction block limit for
/// instructions with per-instruction guards. Probe the largest block that
/// still compiles, publishes and runs one instruction.
fn largest_compilable_block() -> usize {
    for n in (1..=31).rev() {
        let code = not_block(n);
        let view = CodeView::new(0x7F00, &code).unwrap();
        let admission = NativeRegisterV1Admission::from_code(view).unwrap();
        let mut state = LongModeState::default();
        state.rip = 0x7F00;
        state.rflags = 0x246;
        let stop = AtomicU8::new(0);
        let probe = od_jit::dispatch::run_with_native_register_v1(
            view,
            &admission,
            &mut state,
            1,
            DispatchControl {
                stop_rip: None,
                stop_requested: &stop,
                event_requested: None,
            },
            &mut PublishedBlockExecutor,
        );
        match probe {
            Ok(_) => return n + 1,
            Err(DispatchError::Compile(BlockCompileError::EmittedCodeTooLarge { .. })) => {}
            Err(other) => panic!("unexpected probe failure for {n} NOT + JZ: {other:?}"),
        }
    }
    panic!("no block compiles");
}

fn configs() -> Vec<Config> {
    // ADD RAX,RCX ; JZ entry. RAX = RCX = 0 keeps ZF set, the loop never ends.
    let loop2 = vec![0x48, 0x03, 0xC1, 0x74, 0xFB];
    let insns = largest_compilable_block();
    vec![
        Config {
            name: "loop2",
            description: "2-instruction block (ADD;JZ) re-entered by the cached dispatcher"
                .to_owned(),
            code: loop2,
        },
        Config {
            name: "blockmax",
            description: format!(
                "largest block the compile pipeline accepts ({insns} instructions: {} NOT RAX + JZ) re-entered by the cached dispatcher",
                insns - 1
            ),
            code: not_block(insns - 1),
        },
    ]
}

fn measure(config: &Config, runs: usize) -> Vec<u64> {
    let base = 0x7F00;
    let view = CodeView::new(base, &config.code).unwrap();
    let admission = NativeRegisterV1Admission::from_code(view).unwrap();
    let nz = |value| NonZeroU64::new(value).unwrap();
    let dependencies = [CodeDependencySpan {
        start_rip: view.base_rip(),
        end_rip: view.code_end(),
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(nz(91)),
            mapping: MappingId::new(nz(92)),
            mapping_version: MappingVersion::new(nz(1)),
            backing: BackingId::new(nz(93)),
            backing_version: BackingVersion::new(nz(1)),
            executable: true,
        },
    }];
    let context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(2).unwrap(),
    );
    let stop = AtomicU8::new(0);
    let blocks = AtomicU64::new(0);
    let (go_tx, go_rx) = mpsc::channel::<bool>();
    let (done_tx, done_rx) = mpsc::channel::<Result<(Instant, DispatchExit, u64), String>>();
    let mut samples = Vec::with_capacity(runs);

    let stop_ref = &stop;
    let blocks_ref = &blocks;
    struct Shutdown(mpsc::Sender<bool>);
    impl Drop for Shutdown {
        fn drop(&mut self) {
            let _ = self.0.send(false);
        }
    }
    std::thread::scope(|scope| {
        let go_tx = Shutdown(go_tx);
        scope.spawn(move || {
            let mut cache =
                SingleEntryBlockCache::new(1, od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES)
                    .unwrap();
            let mut backend = CountingBackend {
                inner: PublishedBlockExecutor,
                blocks: blocks_ref,
            };
            while go_rx.recv().unwrap_or(false) {
                let mut state = LongModeState::default();
                state.rip = base;
                state.rflags = 0x246; // ZF set: JZ is taken
                let result = run_cached_native_register_v1_controlled(
                    view,
                    &admission,
                    context,
                    &dependencies,
                    &mut state,
                    1 << 62,
                    DispatchControl {
                        stop_rip: None,
                        stop_requested: stop_ref,
                        event_requested: None,
                    },
                    &mut cache,
                    &mut backend,
                );
                let t1 = Instant::now();
                let message = result
                    .map(|result| (t1, result.exit, result.retired_native))
                    .map_err(|error| format!("{error:?}"));
                done_tx.send(message).unwrap();
            }
        });

        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..runs {
            stop.store(0, Ordering::Release);
            let start_blocks = blocks.load(Ordering::Relaxed);
            go_tx.0.send(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while blocks.load(Ordering::Relaxed) < start_blocks + 1000 {
                if let Ok(early) = done_rx.try_recv() {
                    panic!("worker ended before STOP: {early:?}");
                }
                assert!(Instant::now() < deadline, "worker never became busy");
                std::hint::spin_loop();
            }
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let jitter = Duration::from_micros((seed >> 33) % 200);
            let until = Instant::now() + jitter;
            while Instant::now() < until {
                std::hint::spin_loop();
            }
            let t0 = Instant::now();
            stop.store(1, Ordering::Release);
            let (t1, exit, retired) = done_rx.recv().unwrap().expect("dispatch error");
            assert_eq!(exit, DispatchExit::Runtime(RuntimeExitTag::StopRequested));
            assert!(retired > 1000, "the worker was busy before STOP");
            samples.push(t1.saturating_duration_since(t0).as_nanos() as u64);
        }
    });
    samples
}

fn command_output(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn percentile(sorted: &[u64], numerator: usize, denominator: usize) -> u64 {
    // Nearest-rank percentile.
    let rank = (sorted.len() * numerator).div_ceil(denominator).max(1);
    sorted[rank - 1]
}

#[test]
#[ignore = "S3-28d local wall-time measurement; run explicitly (see the file header)"]
fn s3_28d_local_stop_latency_on_the_busy_cached_register_path() {
    let runs: usize = std::env::var("OD_S3_28D_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1000);
    let out_dir = std::env::var_os("OD_S3_28D_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reports/data/stage3/s3-28d")
        });
    std::fs::create_dir_all(&out_dir).unwrap();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let machine = command_output("uname", &["-srm"]);
    let cpu = command_output("sysctl", &["-n", "machdep.cpu.brand_string"]);

    for config in configs() {
        let samples = measure(&config, runs);
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        let sum: u128 = sorted.iter().map(|value| u128::from(*value)).sum();
        let mean = (sum / sorted.len() as u128) as u64;
        let summary = format!(
            "runs={} min_ns={} median_ns={} p95_ns={} p99_ns={} max_ns={} mean_ns={}",
            sorted.len(),
            sorted[0],
            percentile(&sorted, 1, 2),
            percentile(&sorted, 95, 100),
            percentile(&sorted, 99, 100),
            sorted[sorted.len() - 1],
            mean
        );
        println!("[{} / {profile}] {summary}", config.name);

        let path = out_dir.join(format!("stop_latency_{}_{profile}.txt", config.name));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(
            file,
            "# S3-28d STOP latency, LOCAL measurement (not Mini, no millisecond promise)"
        )
        .unwrap();
        writeln!(file, "# config: {} - {}", config.name, config.description).unwrap();
        writeln!(file, "# profile: {profile}; unix_seconds: {unix_seconds}").unwrap();
        writeln!(file, "# machine: {machine}").unwrap();
        writeln!(file, "# cpu: {cpu}").unwrap();
        writeln!(
            file,
            "# command: cargo test -p od-host {}--offline --test s3_28d_stop_latency -- --ignored --nocapture --test-threads=1",
            if profile == "release" { "--release " } else { "" }
        )
        .unwrap();
        writeln!(file, "# protocol: t0 = Instant::now() before the Release store of stop_requested; t1 = Instant::now() after the dispatcher returned StopRequested; latency = t1 - t0 in ns").unwrap();
        writeln!(file, "# summary: {summary}").unwrap();
        writeln!(file, "# raw values (ns), one per run, in run order:").unwrap();
        for value in &samples {
            writeln!(file, "{value}").unwrap();
        }
    }
}
