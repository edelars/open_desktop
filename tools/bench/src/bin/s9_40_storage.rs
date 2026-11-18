//! S9-40: cost of the correct storage operations on host files.
//!
//! `s9_40_storage <empty-dir> [runs]` measures, through the production host
//! backend (`HostFileStorage`: exclusive `flock`, `F_FULLFSYNC`) and the
//! storage library, with every durability barrier kept:
//!
//! * `write`: RAW in place against the prototype CoW (`CowFile`) on a
//!   16 MiB prefilled disk, 4 KiB random writes, one barrier/commit per group
//!   of G = 32 and G = 1 writes; MB/s and write amplification at the backend
//!   call level (bytes accepted by `write_at` / user bytes);
//! * `open`: `CowFile::open` of the 16 MiB file (CRC of every reachable block);
//! * `chain`: a base `SnapFile` of 2 MiB and children up to depth 8; child d
//!   writes a prefix so that each eighth of the disk is finally served by a
//!   different layer; open and a full read of the top layer;
//! * `export`: `export_raw` of the top layer at depth 0 and 8 into a new RAW
//!   (durable, read back and compared);
//! * `compact`: a base with 8 generations of full overwrites, 7 snapshots
//!   deleted, then `compact_to`; time and file size before/after.
//!
//! Every scenario checks its result against an in-memory model and panics on
//! a mismatch. One short warm-up, then `runs` runs (default 5); run 1 is
//! reported as cold, all runs together in p50/mean/sd/min/max. Output is CSV
//! on stdout (`scenario,metric,run,value`) followed by a summary. Files are
//! created inside `<empty-dir>` and removed after each scenario.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    macos::main();
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("s9_40_storage: only macOS/aarch64 has the measured host backend");
    std::process::exit(2);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod macos {
    use od_host::storage_io::HostFileStorage;
    use od_host::storage_policy::{ParentLease, ParentLeaseStorage};
    use od_storage::cow_export::{ExportOptions, StateSelection, export_raw};
    use od_storage::cow_proto::{BLOCK_SIZE, CowFile};
    use od_storage::cow_snapshot::{
        MAX_DISK_BLOCKS, ParentRef, ParentResolver, ResolveError, SnapFile,
    };
    use od_storage::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Instant;

    const BS: usize = BLOCK_SIZE;
    const OP: usize = 4096;
    const DISK: usize = 16 << 20;
    const SNAP_DISK: usize = MAX_DISK_BLOCKS as usize * BS;
    const CHUNK: usize = 16 << 10;

    // ------------------------------------------------------------ plumbing

    /// Counts bytes accepted by `write_at` and durable barriers.
    struct Counting {
        inner: HostFileStorage,
        written: Rc<Cell<u64>>,
        barriers: Rc<Cell<u64>>,
    }

    impl StorageBackend for Counting {
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
            let n = self.inner.write_at(offset, buf)?;
            self.written.set(self.written.get() + n as u64);
            Ok(n)
        }
        fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
            self.inner.set_len(len)
        }
        fn flush_file_durable(&mut self) -> Result<(), BackendError> {
            self.barriers.set(self.barriers.get() + 1);
            self.inner.flush_file_durable()
        }
    }

    struct Counters {
        written: Rc<Cell<u64>>,
        barriers: Rc<Cell<u64>>,
    }

    fn counted(path: &Path) -> (CheckedStorage<Counting>, Counters) {
        let inner = HostFileStorage::create_new_exclusive(path, 0).expect("create bench file");
        let (written, barriers) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let c = Counting {
            inner,
            written: Rc::clone(&written),
            barriers: Rc::clone(&barriers),
        };
        (
            CheckedStorage::new(c).expect("capabilities"),
            Counters { written, barriers },
        )
    }

    fn fresh(path: &Path) -> CheckedStorage<HostFileStorage> {
        CheckedStorage::new(HostFileStorage::create_new_exclusive(path, 0).expect("create"))
            .expect("capabilities")
    }

    fn writer(path: &Path) -> CheckedStorage<HostFileStorage> {
        CheckedStorage::new(HostFileStorage::open_read_write_exclusive(path).expect("open"))
            .expect("capabilities")
    }

    /// Parents by uid, opened under a read lease as the offline CLI does.
    struct Paths(BTreeMap<u64, PathBuf>);

    impl ParentResolver for Paths {
        type Backend = ParentLeaseStorage;
        fn open_parent(
            &mut self,
            p: &ParentRef,
        ) -> Result<CheckedStorage<ParentLeaseStorage>, ResolveError> {
            let path = self.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
            let lease = ParentLease::open(path).map_err(|_| ResolveError::Busy)?;
            CheckedStorage::new(ParentLeaseStorage::new(lease)).map_err(ResolveError::Storage)
        }
    }

    /// SplitMix64: deterministic payload and offsets.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
        /// A permutation of `0..n`.
        fn perm(&mut self, n: usize) -> Vec<usize> {
            let mut v: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                v.swap(i, (self.next() % (i as u64 + 1)) as usize);
            }
            v
        }
    }

    struct Out {
        rows: Vec<(String, String, usize, f64)>,
    }

    impl Out {
        fn put(&mut self, scenario: &str, metric: &str, run: usize, value: f64) {
            println!("{scenario},{metric},{run},{value:.4}");
            self.rows
                .push((scenario.to_string(), metric.to_string(), run, value));
        }
    }

    fn ms(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1e3
    }

    fn file_len(p: &Path) -> u64 {
        std::fs::metadata(p).expect("metadata").len()
    }

    // ----------------------------------------------------------- scenarios

    /// RAW in place against CoW, 4 KiB random writes over a prefilled disk.
    fn write(dir: &Path, disk: usize, group: usize, data: usize, run: usize, out: &mut Out) {
        let mut rng = Rng(0x5940_0000 + run as u64);
        let prefill = rng.bytes(disk);
        let offs: Vec<usize> = rng.perm(disk / OP)[..data / OP].to_vec();
        let payload = rng.bytes(data);
        let mut model = prefill.clone();
        for (i, o) in offs.iter().enumerate() {
            model[o * OP..(o + 1) * OP].copy_from_slice(&payload[i * OP..(i + 1) * OP]);
        }
        let name = format!("write_g{group}_{}m", data >> 20);

        // RAW
        let p = dir.join("raw.img");
        let (mut s, c) = counted(&p);
        s.set_len(disk as u64).expect("size");
        s.write_all_at(0, &prefill).expect("prefill");
        s.flush_file_durable().expect("barrier");
        let (w0, b0) = (c.written.get(), c.barriers.get());
        let t = Instant::now();
        for (g, ops) in offs.chunks(group).enumerate() {
            for (k, o) in ops.iter().enumerate() {
                let i = g * group + k;
                s.write_all_at((o * OP) as u64, &payload[i * OP..(i + 1) * OP])
                    .expect("write");
            }
            s.flush_file_durable().expect("barrier");
        }
        let el = t.elapsed().as_secs_f64();
        out.put(&format!("{name}_raw"), "mb_s", run, data as f64 / el / 1e6);
        out.put(
            &format!("{name}_raw"),
            "wa_backend",
            run,
            (c.written.get() - w0) as f64 / data as f64,
        );
        out.put(
            &format!("{name}_raw"),
            "barriers",
            run,
            (c.barriers.get() - b0) as f64,
        );
        drop(s);
        let mut back = vec![0u8; disk];
        CheckedStorage::new(HostFileStorage::open_read_only(&p).expect("ro"))
            .expect("caps")
            .read_exact_at(0, &mut back)
            .expect("read");
        assert!(back == model, "RAW result differs from the model");
        std::fs::remove_file(&p).expect("rm");

        // CoW
        let p = dir.join("cow.img");
        let (s, c) = counted(&p);
        let (mut f, _) = CowFile::create(s).expect("create");
        for blocks in prefill.chunks(256 * BS).enumerate().collect::<Vec<_>>() {
            let (k, chunk) = blocks;
            let writes: Vec<(u64, &[u8])> = chunk
                .chunks(BS)
                .enumerate()
                .map(|(j, b)| ((k * 256 + j) as u64, b))
                .collect();
            f.commit(&writes).expect("prefill commit");
        }
        let (w0, b0, l0) = (c.written.get(), c.barriers.get(), file_len(&p));
        let t = Instant::now();
        for (g, ops) in offs.chunks(group).enumerate() {
            let mut writes: Vec<(u64, &[u8])> = Vec::with_capacity(ops.len() * OP / BS);
            for (k, o) in ops.iter().enumerate() {
                let i = g * group + k;
                for j in 0..OP / BS {
                    let at = i * OP + j * BS;
                    writes.push(((o * OP / BS + j) as u64, &payload[at..at + BS]));
                }
            }
            f.commit(&writes).expect("commit");
        }
        let el = t.elapsed().as_secs_f64();
        let n = format!("{name}_cow");
        out.put(&n, "mb_s", run, data as f64 / el / 1e6);
        out.put(
            &n,
            "wa_backend",
            run,
            (c.written.get() - w0) as f64 / data as f64,
        );
        out.put(&n, "wa_file", run, (file_len(&p) - l0) as f64 / data as f64);
        out.put(&n, "barriers", run, (c.barriers.get() - b0) as f64);
        drop(f);
        let t = Instant::now();
        let (f, _) = CowFile::open(writer(&p)).expect("reopen");
        out.put(&n, "open_ms", run, ms(t));
        let back: Vec<u8> = f.snapshot().expect("read").concat();
        assert!(back == model, "CoW result differs from the model");
        drop(f);
        std::fs::remove_file(&p).expect("rm");
    }

    /// Base of `SNAP_DISK` plus children up to `depth`. With `prefix` each
    /// child writes a prefix (see the loop), so the data volume of the chain
    /// grows with depth; without it the children are empty (only a snapshot),
    /// so the volume stays one base and every block is read from the base
    /// through all levels. Returns the paths (base first) and the model.
    fn build_chain(
        dir: &Path,
        depth: usize,
        prefix: bool,
        rng: &mut Rng,
    ) -> (Vec<PathBuf>, Vec<u8>, Paths) {
        let mut model = rng.bytes(SNAP_DISK);
        let mut paths = Vec::new();
        let mut resolver = Paths(BTreeMap::new());
        let base = dir.join("layer0.img");
        let (mut f, _) = SnapFile::create_base(fresh(&base), 1).expect("base");
        for (k, c) in model.chunks(CHUNK).enumerate() {
            f.write_at((k * CHUNK) as u64, c).expect("write base");
        }
        let (mut id, _) = f.create_snapshot().expect("snapshot");
        let mut parent = f.snapshot_ref(id).expect("ref");
        drop(f);
        resolver.0.insert(1, base.clone());
        paths.push(base);
        for d in 1..=depth {
            let p = dir.join(format!("layer{d}.img"));
            let uid = d as u64 + 1;
            let (mut f, _) =
                SnapFile::create_child(fresh(&p), &mut resolver, &parent, uid).expect("child");
            // A layer file allocates every block below its highest write, so
            // each child writes a prefix from offset 0: layer d covers eighths
            // 0..=depth-d, and eighth r is finally read from layer depth-r.
            if prefix {
                let len = (depth - d + 1) * (SNAP_DISK / 8);
                let data = rng.bytes(len);
                for (k, c) in data.chunks(CHUNK).enumerate() {
                    f.write_at((k * CHUNK) as u64, c).expect("write child");
                }
                model[..len].copy_from_slice(&data);
            }
            (id, _) = f.create_snapshot().expect("snapshot");
            parent = f.snapshot_ref(id).expect("ref");
            drop(f);
            resolver.0.insert(uid, p.clone());
            paths.push(p);
        }
        (paths, model, resolver)
    }

    fn chain(dir: &Path, run: usize, out: &mut Out) {
        let layouts = [
            (true, "chain_d", &[0usize, 1, 2, 4, 8][..]),
            (false, "chain_empty_d", &[1, 2, 4, 8][..]),
        ];
        for (prefix, label, depths) in layouts {
            for &depth in depths {
                let mut rng = Rng(0x5940_1000 + run as u64 * 16 + depth as u64);
                let (paths, model, mut resolver) = build_chain(dir, depth, prefix, &mut rng);
                let top = paths.last().expect("top").clone();
                let n = format!("{label}{depth}");
                let t = Instant::now();
                let (f, _) = SnapFile::open(writer(&top), &mut resolver).expect("open top");
                out.put(&n, "open_ms", run, ms(t));
                let mut back = vec![0u8; SNAP_DISK];
                let t = Instant::now();
                f.read_at(0, &mut back).expect("read");
                let el = t.elapsed().as_secs_f64();
                out.put(&n, "read_mb_s", run, SNAP_DISK as f64 / el / 1e6);
                assert!(back == model, "chain depth {depth} differs from the model");
                drop(f);
                if prefix && (depth == 0 || depth == 8) {
                    let raw = dir.join("export.raw");
                    let src = CheckedStorage::new(
                        od_host::storage_io::ReadLease::open(&top).expect("lease"),
                    )
                    .expect("caps");
                    let t = Instant::now();
                    export_raw(
                        src,
                        &mut resolver,
                        StateSelection::Head,
                        || Ok(fresh(&raw)),
                        &ExportOptions::default(),
                        &|| false,
                    )
                    .expect("export");
                    let el = t.elapsed().as_secs_f64();
                    out.put(&format!("export_d{depth}"), "ms", run, el * 1e3);
                    out.put(
                        &format!("export_d{depth}"),
                        "effective_mb_s",
                        run,
                        SNAP_DISK as f64 / el / 1e6,
                    );
                    assert!(std::fs::read(&raw).expect("raw") == model, "export differs");
                    std::fs::remove_file(&raw).expect("rm");
                }
                for p in paths {
                    std::fs::remove_file(p).expect("rm");
                }
            }
        }
    }

    fn compact(dir: &Path, run: usize, out: &mut Out) {
        let mut rng = Rng(0x5940_2000 + run as u64);
        let p = dir.join("gen.img");
        let (mut f, _) = SnapFile::create_base(fresh(&p), 1).expect("base");
        let mut model = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..8 {
            model = rng.bytes(SNAP_DISK);
            for (k, c) in model.chunks(CHUNK).enumerate() {
                f.write_at((k * CHUNK) as u64, c).expect("write");
            }
            ids.push(f.create_snapshot().expect("snapshot").0);
        }
        for id in &ids[..7] {
            f.delete_snapshot(*id).expect("delete");
        }
        let before = file_len(&p);
        let q = dir.join("compacted.img");
        let target = fresh(&q);
        let t = Instant::now();
        let (g, _) = f.compact_to(target, 2).expect("compact");
        out.put("compact_8gen", "ms", run, ms(t));
        out.put("compact_8gen", "file_before_mb", run, before as f64 / 1e6);
        out.put(
            "compact_8gen",
            "file_after_mb",
            run,
            file_len(&q) as f64 / 1e6,
        );
        let mut back = vec![0u8; SNAP_DISK];
        g.read_at(0, &mut back).expect("read");
        assert!(back == model, "compacted head differs");
        let mut snap = vec![0u8; SNAP_DISK];
        g.read_snapshot_at(ids[7], 0, &mut snap)
            .expect("snapshot read");
        assert!(snap == model, "kept snapshot differs");
        assert_eq!(g.snapshot_ids(), vec![ids[7]], "deleted snapshots survived");
        drop((f, g));
        std::fs::remove_file(&p).expect("rm");
        std::fs::remove_file(&q).expect("rm");
    }

    fn rss_kb() -> f64 {
        let o = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .expect("ps");
        String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(f64::NAN)
    }

    fn summary(out: &Out) {
        let mut keys: Vec<(String, String)> = out
            .rows
            .iter()
            .filter(|r| r.2 > 0)
            .map(|r| (r.0.clone(), r.1.clone()))
            .collect();
        keys.sort();
        keys.dedup();
        println!("\n# summary: scenario,metric,n,cold,p50,mean,sd,min,max");
        for (s, m) in keys {
            let mut v: Vec<(usize, f64)> = out
                .rows
                .iter()
                .filter(|r| r.0 == s && r.1 == m && r.2 > 0)
                .map(|r| (r.2, r.3))
                .collect();
            v.sort_by_key(|x| x.0);
            let cold = v.first().map_or(f64::NAN, |x| x.1);
            let mut x: Vec<f64> = v.iter().map(|x| x.1).collect();
            x.sort_by(f64::total_cmp);
            let n = x.len() as f64;
            let mean = x.iter().sum::<f64>() / n;
            let sd =
                (x.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
            let p50 = if x.len() % 2 == 1 {
                x[x.len() / 2]
            } else {
                (x[x.len() / 2 - 1] + x[x.len() / 2]) / 2.0
            };
            println!(
                "{s},{m},{},{cold:.3},{p50:.3},{mean:.3},{sd:.3},{:.3},{:.3}",
                x.len(),
                x[0],
                x[x.len() - 1]
            );
        }
    }

    /// One line describing the host, the volume of `dir` and the build, so a
    /// result file names its own conditions.
    fn environment(dir: &Path) -> String {
        let cmd = |c: &str, a: &[&str]| {
            std::process::Command::new(c)
                .args(a)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "?".to_string())
        };
        let df = std::process::Command::new("df")
            .args(["-h"])
            .arg(dir)
            .output()
            .map(|o| {
                let text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.lines()
                    .last()
                    .unwrap_or("?")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|_| "?".to_string());
        format!(
            "date={} macos={} build={} model={} cpu={} mem={} power={} df=[{}] head={} profile={}",
            cmd("date", &["-u", "+%FT%TZ"]),
            cmd("sw_vers", &["-productVersion"]),
            cmd("sw_vers", &["-buildVersion"]),
            cmd("sysctl", &["-n", "hw.model"]).replace('\n', " "),
            cmd("sysctl", &["-n", "machdep.cpu.brand_string"]),
            cmd("sysctl", &["-n", "hw.memsize"]),
            cmd("pmset", &["-g", "batt"]).lines().next().unwrap_or("?"),
            df,
            cmd("git", &["describe", "--always", "--dirty"]),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        )
    }

    pub fn main() {
        let mut args = std::env::args().skip(1);
        let (Some(dir), runs) = (args.next(), args.next()) else {
            eprintln!("usage: s9_40_storage <empty-dir> [runs]");
            std::process::exit(2);
        };
        let dir = PathBuf::from(dir);
        let Ok(runs) = runs.map_or(Ok(5), |r| r.parse::<usize>()) else {
            eprintln!("usage: s9_40_storage <empty-dir> [runs]");
            std::process::exit(2);
        };
        assert!(
            std::fs::read_dir(&dir).expect("dir").next().is_none(),
            "the directory must exist and be empty"
        );
        let mut out = Out { rows: Vec::new() };
        println!("# env {}", environment(&dir));
        println!("scenario,metric,run,value");
        out.put("process", "rss_kb_start", 0, rss_kb());
        // Warm-up (run 0, not summarised): small disk, every code path once.
        write(&dir, 2 << 20, 32, 1 << 20, 0, &mut out);
        chain(&dir, 0, &mut out);
        compact(&dir, 0, &mut out);
        for run in 1..=runs {
            write(&dir, DISK, 32, DISK, run, &mut out);
            write(&dir, DISK, 1, 1 << 20, run, &mut out);
            chain(&dir, run, &mut out);
            compact(&dir, run, &mut out);
            out.put("process", "rss_kb_run_end", run, rss_kb());
        }
        summary(&out);
    }
}
