//! Runs the real binary against image files and checks the exit statuses, the
//! bounded output and that the source is never changed (S9-19).

use od_host::storage_io::HostFileStorage;
use od_image_tool::{Exit, MAX_REPORT_BYTES};
use od_storage::cow_proto::crc32;
use od_storage::cow_snapshot::{NoParents, ParentRef, ParentResolver, ResolveError, SnapFile};
use od_storage::io::CheckedStorage;
use od_storage::testing::fault::{CrashPolicy, FaultBackend, FaultHandle};
use od_storage::testing::rng::SplitMix64;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const BS: usize = 512;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "od-image-tool-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).expect("temp dir");
        Self(p)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).expect("write image");
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn st(h: &FaultHandle) -> CheckedStorage<FaultBackend> {
    CheckedStorage::new(h.open()).expect("checked")
}

struct Handles(BTreeMap<u64, FaultHandle>);

impl ParentResolver for Handles {
    type Backend = FaultBackend;

    fn open_parent(&mut self, p: &ParentRef) -> Result<CheckedStorage<FaultBackend>, ResolveError> {
        let h = self.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        CheckedStorage::new(h.open()).map_err(ResolveError::Storage)
    }
}

/// Image bytes of a base file (uid `uid`): `payload`, snapshot 1, overwrite.
fn base_image(uid: u64, payload: &[u8]) -> (FaultHandle, Vec<u8>) {
    let h = FaultHandle::new(&[], uid);
    let (mut f, _) = SnapFile::create_base(st(&h), uid).expect("create");
    f.write_at(0, payload).expect("write");
    f.create_snapshot().expect("snapshot");
    f.write_at(0, &SplitMix64(5).bytes(BS)).expect("overwrite");
    drop(f);
    h.crash(CrashPolicy::DropPending);
    let bytes = h.durable_bytes();
    (h, bytes)
}

fn tool(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_od-image-tool"))
        .args(args)
        .output()
        .expect("run tool");
    (
        out.status.code().expect("exit code"),
        String::from_utf8(out.stdout).expect("utf8 stdout"),
        String::from_utf8(out.stderr).expect("utf8 stderr"),
    )
}

fn p(path: &Path) -> &str {
    path.to_str().expect("utf8 path")
}

#[cfg(target_os = "macos")]
#[test]
fn create_raw_creates_only_a_new_zeroed_file() {
    let dir = Dir::new();
    let (code, out, err) = tool(&["create-raw", p(&dir.0), "fresh.raw", "4096"]);
    assert_eq!(code, i32::from(Exit::Ok.code()), "{out}{err}");
    assert_eq!(out, "created=1\ncapacity_bytes=4096\n");
    assert!(err.is_empty());
    let image = std::fs::read(dir.0.join("fresh.raw")).expect("created RAW");
    assert_eq!(image.len(), 4096);
    assert!(image.iter().all(|byte| *byte == 0));
}

#[cfg(target_os = "macos")]
#[test]
fn create_raw_refuses_an_existing_file_without_changing_it() {
    let dir = Dir::new();
    let path = dir.file("existing.raw", b"preserve this file");
    let before = std::fs::read(&path).expect("read original");
    let (code, out, err) = tool(&["create-raw", p(&dir.0), "existing.raw", "4096"]);
    assert_eq!(code, i32::from(Exit::Io.code()), "{out}{err}");
    assert!(out.is_empty());
    assert_eq!(err, "create_failed=status_3\n");
    assert_eq!(
        std::fs::read(&path).expect("read original after refusal"),
        before
    );
}

#[cfg(target_os = "macos")]
#[test]
fn create_raw_rejects_a_fifo_directory_without_blocking() {
    let dir = Dir::new();
    let fifo = dir.0.join("not-a-directory");
    let status = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
    assert!(status.success());

    let mut child = Command::new(env!("CARGO_BIN_EXE_od-image-tool"))
        .args(["create-raw", p(&fifo), "fresh.raw", "4096"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn create-raw");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(output) = child.try_wait().expect("poll child") {
            assert_eq!(output.code(), Some(i32::from(Exit::Io.code())));
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("create-raw blocked while opening a FIFO as its directory");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!dir.0.join("fresh.raw").exists());
}

#[test]
fn a_valid_image_exits_zero_and_is_not_changed() {
    let dir = Dir::new();
    let (_, bytes) = base_image(1, &SplitMix64(1).bytes(2 * BS));
    let path = dir.file("a.img", &bytes);
    for cmd in ["info", "check"] {
        let (code, out, err) = tool(&[cmd, p(&path)]);
        assert_eq!(code, i32::from(Exit::Ok.code()), "{cmd}: {out}{err}");
        assert!(
            out.contains("outcome=ok\n") && out.contains("snapshots=1\n"),
            "{out}"
        );
        assert!(
            out.contains("logical_size=1024\n") && out.contains("kind=base\n"),
            "{out}"
        );
        assert!(out.len() <= MAX_REPORT_BYTES && out.is_ascii());
        assert!(err.is_empty());
    }
    assert_eq!(
        std::fs::read(&path).expect("read"),
        bytes,
        "the image changed"
    );
}

#[test]
fn a_corrupt_block_exits_two_and_the_file_is_left_alone() {
    let dir = Dir::new();
    let (_, mut bytes) = base_image(1, &SplitMix64(1).bytes(2 * BS));
    // Corrupt a block the head references: the newest data sits near the end.
    let at = bytes.len() - 512 - 12 * 6 - 700;
    bytes[at] ^= 0x55;
    let path = dir.file("bad.img", &bytes);
    let (code, out, _) = tool(&["check", p(&path)]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("outcome=corrupt\n"), "{out}");
    assert_eq!(std::fs::read(&path).expect("read"), bytes, "silent repair");
}

#[test]
fn an_unsupported_mandatory_feature_exits_three() {
    let dir = Dir::new();
    let (_, mut bytes) = base_image(1, &SplitMix64(1).bytes(BS));
    for slot in 0..2 {
        let at = slot * 512;
        bytes[at + 52..at + 60].copy_from_slice(&(1u64 << 40).to_le_bytes());
        let crc = crc32(&bytes[at..at + 508]);
        bytes[at + 508..at + 512].copy_from_slice(&crc.to_le_bytes());
    }
    let path = dir.file("future.img", &bytes);
    let (code, out, _) = tool(&["info", p(&path)]);
    assert_eq!(code, 3, "{out}");
    assert!(
        out.contains("outcome=unsupported\n")
            && out.contains("reason=unsupported_mandatory_feature\n"),
        "{out}"
    );
    assert!(out.contains("detail=40\n"), "{out}");
    // Garbage that is not an image at all is corrupt, not unsupported.
    let junk = dir.file("junk.img", &[0xEE; 4096]);
    assert_eq!(tool(&["check", p(&junk)]).0, 2);
}

#[test]
fn a_file_held_by_a_writer_exits_four_and_a_missing_file_five() {
    let dir = Dir::new();
    let (_, bytes) = base_image(1, &SplitMix64(1).bytes(BS));
    let path = dir.file("busy.img", &bytes);
    let writer = HostFileStorage::open_read_write_exclusive(&path).expect("writer");
    let (code, out, _) = tool(&["check", p(&path)]);
    assert_eq!(code, 4, "{out}");
    assert!(out.contains("outcome=busy\n"), "{out}");
    drop(writer);
    assert_eq!(
        tool(&["check", p(&path)]).0,
        0,
        "free after the writer left"
    );

    let missing = dir.0.join("nope.img");
    let (code, out, _) = tool(&["info", p(&missing)]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains("reason=not_found\n"), "{out}");
    // A directory is not a regular file.
    assert_eq!(tool(&["info", p(&dir.0)]).0, 5);
}

#[test]
fn usage_errors_exit_one_and_read_nothing() {
    let dir = Dir::new();
    let (_, bytes) = base_image(1, &SplitMix64(1).bytes(BS));
    let path = dir.file("a.img", &bytes);
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["frobnicate", p(&path)],
        vec!["check"],
        vec!["info"],
        vec!["check", p(&path), "extra"],
        vec!["check", p(&path), "--parent"],
        vec!["check", p(&path), "--parent", "nouid"],
        vec!["check", p(&path), "--parent", "x=/tmp/y"],
        vec!["check", p(&path), "--wat"],
        vec!["info", p(&path), "--parent", "1=/tmp/y"],
    ];
    for args in cases {
        let (code, out, err) = tool(&args);
        assert_eq!(code, 1, "{args:?}: {out}{err}");
        assert!(out.is_empty(), "{args:?}");
        assert!(err.contains("usage:"), "{args:?}: {err}");
    }
}

#[test]
fn a_child_needs_its_parent_and_the_parent_must_be_the_recorded_one() {
    let dir = Dir::new();
    let (hb, base_bytes) = base_image(1, &SplitMix64(1).bytes(2 * BS));
    let base = dir.file("base.img", &base_bytes);
    let (b, _) = SnapFile::open(st(&hb), &mut NoParents).expect("base");
    let pref = b.snapshot_ref(1).expect("ref");
    drop(b);
    hb.crash(CrashPolicy::DropPending);
    let hc = FaultHandle::new(&[], 2);
    let mut handles = Handles(BTreeMap::from([(1, hb.clone())]));
    let (mut c, _) = SnapFile::create_child(st(&hc), &mut handles, &pref, 2).expect("child");
    c.write_at(3, &[1, 2, 3]).expect("write");
    drop(c);
    hc.crash(CrashPolicy::DropPending);
    let child_bytes = hc.durable_bytes();
    let child = dir.file("child.img", &child_bytes);
    let spec = format!("1={}", p(&base));

    // info lists the recorded link without verifying it.
    let (code, out, _) = tool(&["info", p(&child)]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("kind=child\n") && out.contains("chain=1:1\n"),
        "{out}"
    );
    assert!(out.contains("chain_verified=false\n"), "{out}");
    // check without the parent refuses as an open would.
    let (code, out, _) = tool(&["check", p(&child)]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("reason=parent_missing\n"), "{out}");
    // With the right parent it verifies the chain.
    let (code, out, _) = tool(&["check", p(&child), "--parent", &spec]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("chain_verified=true\n") && out.contains("chain=1:1\n"),
        "{out}"
    );
    // Another file under the same name is refused.
    let (_, other_bytes) = base_image(77, &SplitMix64(2).bytes(2 * BS));
    let other = dir.file("other.img", &other_bytes);
    let swapped = format!("1={}", p(&other));
    let (code, out, _) = tool(&["check", p(&child), "--parent", &swapped]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("reason=parent_identity_mismatch\n"), "{out}");
    // A parent held by a writer is busy.
    let writer = HostFileStorage::open_read_write_exclusive(&base).expect("writer");
    assert_eq!(tool(&["check", p(&child), "--parent", &spec]).0, 4);
    drop(writer);
    assert_eq!(std::fs::read(&base).expect("base"), base_bytes);
    assert_eq!(std::fs::read(&child).expect("child"), child_bytes);
}

#[test]
fn diagnostics_never_contain_guest_payload_or_paths() {
    let dir = Dir::new();
    let marker = b"TOP-SECRET-GUEST-PAYLOAD-0123456789";
    let mut payload = Vec::new();
    while payload.len() < 2 * BS {
        payload.extend_from_slice(marker);
    }
    let (_, mut bytes) = base_image(1, &payload);
    let path = dir.file("secret-name-image.img", &bytes);
    let (_, out, err) = tool(&["check", p(&path)]);
    assert!(!out.contains("SECRET") && !err.contains("SECRET"));
    // Also for a corrupt image (the failure path).
    let at = bytes.len() - 512 - 12 * 6 - 700;
    bytes[at] ^= 1;
    let bad = dir.file("secret-bad.img", &bytes);
    let (code, out, err) = tool(&["check", p(&bad)]);
    assert_eq!(code, 2);
    assert!(!out.contains("SECRET") && !out.contains("secret-bad") && !err.contains("secret"));
    let (_, out, err) = tool(&["info", p(&dir.0.join("secret-missing.img"))]);
    assert!(!out.contains("secret-missing") && !err.contains("secret-missing"));
}

// ------------------------------------------------- export and salvage (S9-20/21)

/// Success paths write through the host file backend, which only declares file
/// durability on macOS; elsewhere the tool refuses (`durability_unavailable`)
/// and these tests have nothing to compare.
fn durable_host() -> bool {
    cfg!(target_os = "macos")
}

/// Head model of `base_image(uid, payload)`: block 0 overwritten after the
/// snapshot. Plain bytes, no parser.
fn head_model(payload: &[u8]) -> Vec<u8> {
    let mut v = payload.to_vec();
    v.resize(v.len().div_ceil(BS) * BS, 0);
    v[..BS].copy_from_slice(&SplitMix64(5).bytes(BS));
    v
}

fn snapshot_model(payload: &[u8]) -> Vec<u8> {
    let mut v = payload.to_vec();
    v.resize(v.len().div_ceil(BS) * BS, 0);
    v
}

fn names(dir: &Dir) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&dir.0)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn export_writes_exactly_the_selected_state_and_publishes_only_when_verified() {
    if !durable_host() {
        return;
    }
    let dir = Dir::new();
    let payload = SplitMix64(1).bytes(2 * BS);
    let (_, bytes) = base_image(1, &payload);
    let img = dir.file("a.img", &bytes);

    let out = dir.0.join("head.raw");
    let (code, text, err) = tool(&["export", p(&img), p(&out)]);
    assert_eq!(code, 0, "{text}{err}");
    for want in [
        "command=export\n",
        "outcome=ok\n",
        "selection=head\n",
        "logical_size=1024\n",
        "verified=true\n",
        "file_durable=true\n",
        "published=true\n",
        "namespace_durable=false\n",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(text.len() <= MAX_REPORT_BYTES && text.is_ascii());
    // An independent reader: plain std bytes against a plain model.
    assert_eq!(std::fs::read(&out).expect("read raw"), head_model(&payload));
    assert!(
        !dir.0.join("head.raw.partial").exists(),
        "the partial name must be gone: {:?}",
        names(&dir)
    );

    let snap = dir.0.join("snap1.raw");
    let (code, text, _) = tool(&["export", p(&img), p(&snap), "--snapshot", "1"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("selection=snapshot:1\n"), "{text}");
    assert_eq!(
        std::fs::read(&snap).expect("read raw"),
        snapshot_model(&payload)
    );

    // A snapshot that is not there: corrupt-class refusal, nothing created.
    let none = dir.0.join("snap9.raw");
    let (code, text, _) = tool(&["export", p(&img), p(&none), "--snapshot", "9"]);
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("reason=snapshot_missing\n"), "{text}");
    assert!(text.contains("published=false\n"), "{text}");
    assert!(!none.exists() && !dir.0.join("snap9.raw.partial").exists());
    assert_eq!(std::fs::read(&img).expect("read"), bytes, "source changed");
}

#[test]
fn export_never_replaces_or_reuses_an_existing_name() {
    let dir = Dir::new();
    let (_, bytes) = base_image(1, &SplitMix64(1).bytes(BS));
    let img = dir.file("a.img", &bytes);
    let existing = dir.file("out.raw", b"precious bytes");
    let (code, text, _) = tool(&["export", p(&img), p(&existing)]);
    assert_eq!(code, 5, "{text}");
    assert!(
        text.contains("reason=target_exists\n") && text.contains("published=false\n"),
        "{text}"
    );
    assert_eq!(std::fs::read(&existing).expect("read"), b"precious bytes");
    assert!(!dir.0.join("out.raw.partial").exists());

    // A leftover partial name is not reused either (and never removed).
    let out = dir.0.join("fresh.raw");
    let stale = dir.file("fresh.raw.partial", b"leftover of an old attempt");
    let (code, text, _) = tool(&["export", p(&img), p(&out)]);
    assert_eq!(code, 5, "{text}");
    assert!(text.contains("reason=partial_exists\n"), "{text}");
    assert_eq!(
        std::fs::read(&stale).expect("read"),
        b"leftover of an old attempt"
    );
    assert!(!out.exists());
    assert_eq!(std::fs::read(&img).expect("read"), bytes);
}

#[test]
fn an_unusable_or_busy_source_creates_nothing_and_a_late_failure_is_only_ever_partial() {
    let dir = Dir::new();
    let (hb, bytes) = base_image(1, &SplitMix64(1).bytes(2 * BS));

    // Corrupt head block: the source does not open, so no target is created.
    let mut bad = bytes.clone();
    let at = bad.len() - 512 - 12 * 6 - 700;
    bad[at] ^= 0x55;
    let img = dir.file("bad.img", &bad);
    let out = dir.0.join("bad.raw");
    let (code, text, _) = tool(&["export", p(&img), p(&out)]);
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("partial_left=false\n"), "{text}");
    assert!(!out.exists() && !dir.0.join("bad.raw.partial").exists());

    // A writer holds the source: busy, nothing created.
    let good = dir.file("good.img", &bytes);
    let writer = HostFileStorage::open_read_write_exclusive(&good).expect("writer");
    let out = dir.0.join("busy.raw");
    let (code, text, _) = tool(&["export", p(&good), p(&out)]);
    assert_eq!(code, 4, "{text}");
    assert!(text.contains("outcome=busy\n"), "{text}");
    assert!(!out.exists() && !dir.0.join("busy.raw.partial").exists());
    drop(writer);

    // A block only snapshot 1 reaches is damaged: the image still opens, the
    // export of snapshot 1 fails while reading, and the target name is never
    // created; what remains is the file named `.partial`.
    if !durable_host() {
        return;
    }
    let (f, _) = SnapFile::open(st(&hb), &mut NoParents).expect("open");
    let snap_blocks = f.snapshot_data_blocks(1).expect("snapshot blocks");
    drop(f);
    hb.crash(CrashPolicy::DropPending);
    let view = od_storage::cow_proto::CowView::salvage(st(&hb))
        .expect("view")
        .0;
    let head_blocks: Vec<u64> = (0..view.block_count())
        .filter_map(|i| view.block_offset(i))
        .collect();
    let only: Vec<u64> = snap_blocks
        .into_iter()
        .filter(|b| !head_blocks.contains(b))
        .collect();
    assert_eq!(only.len(), 1, "{only:?}");
    let mut damaged = bytes.clone();
    damaged[only[0] as usize + 4] ^= 0x20;
    let img = dir.file("snapbad.img", &damaged);
    let out = dir.0.join("snapbad.raw");
    let (code, text, _) = tool(&["export", p(&img), p(&out), "--snapshot", "1"]);
    assert_eq!(code, 2, "{text}");
    assert!(
        text.contains("published=false\n") && text.contains("partial_left=true\n"),
        "{text}"
    );
    assert!(
        !out.exists(),
        "an incomplete export must not have the target name"
    );
    assert!(dir.0.join("snapbad.raw.partial").exists());
    // The undamaged head of the same image still exports.
    let head_out = dir.0.join("snapbad-head.raw");
    let (code, text, _) = tool(&["export", p(&img), p(&head_out)]);
    assert_eq!(code, 0, "{text}");
}

#[test]
fn a_child_is_exported_flattened_and_only_with_its_recorded_parent() {
    if !durable_host() {
        return;
    }
    let dir = Dir::new();
    let payload = SplitMix64(1).bytes(2 * BS);
    let (hb, base_bytes) = base_image(1, &payload);
    let base = dir.file("base.img", &base_bytes);
    let (b, _) = SnapFile::open(st(&hb), &mut NoParents).expect("base");
    let pref = b.snapshot_ref(1).expect("ref");
    drop(b);
    hb.crash(CrashPolicy::DropPending);
    let hc = FaultHandle::new(&[], 2);
    let mut handles = Handles(BTreeMap::from([(1, hb.clone())]));
    let (mut c, _) = SnapFile::create_child(st(&hc), &mut handles, &pref, 2).expect("child");
    c.write_at(3, &[1, 2, 3]).expect("write");
    drop(c);
    hc.crash(CrashPolicy::DropPending);
    let child = dir.file("child.img", &hc.durable_bytes());
    let mut model = snapshot_model(&payload);
    model[3..6].copy_from_slice(&[1, 2, 3]);

    let out = dir.0.join("child.raw");
    let (code, text, _) = tool(&["export", p(&child), p(&out)]);
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("reason=parent_missing\n"), "{text}");
    assert!(!out.exists());
    let spec = format!("1={}", p(&base));
    let (code, text, _) = tool(&["export", p(&child), p(&out), "--parent", &spec]);
    assert_eq!(code, 0, "{text}");
    assert_eq!(std::fs::read(&out).expect("read raw"), model);
    assert_eq!(std::fs::read(&base).expect("base"), base_bytes);
}

#[test]
fn salvage_of_an_intact_image_is_complete_and_its_copy_exports_back() {
    if !durable_host() {
        return;
    }
    let dir = Dir::new();
    let payload = SplitMix64(1).bytes(2 * BS);
    let (_, bytes) = base_image(1, &payload);
    let img = dir.file("a.img", &bytes);
    let (copy, rep) = (dir.0.join("copy.img"), dir.0.join("report.txt"));
    let (code, text, err) = tool(&["salvage", p(&img), p(&copy), p(&rep)]);
    assert_eq!(code, 0, "{text}{err}");
    for want in [
        "command=salvage\n",
        "outcome=ok\n",
        "recoverability=complete\n",
        "reason=ok\n",
        "copy=published\n",
        "lost_bytes=0\n",
        "report_file=written\n",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(text.len() <= MAX_REPORT_BYTES && text.is_ascii());
    let report = std::fs::read_to_string(&rep).expect("report file");
    assert!(
        report.contains("recoverability=complete\n") && report.contains("extent=0+1024:valid:"),
        "{report}"
    );
    // The copy is a published image that checks clean and exports back.
    assert_eq!(tool(&["check", p(&copy)]).0, 0);
    let back = dir.0.join("back.raw");
    assert_eq!(tool(&["export", p(&copy), p(&back)]).0, 0);
    assert_eq!(
        std::fs::read(&back).expect("read raw"),
        head_model(&payload)
    );
    assert_eq!(std::fs::read(&img).expect("read"), bytes, "source changed");

    // A snapshot can be salvaged on its own.
    let (copy1, rep1) = (dir.0.join("copy1.img"), dir.0.join("report1.txt"));
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy1), p(&rep1), "--snapshot", "1"]);
    assert_eq!(code, 0, "{text}");
    let back1 = dir.0.join("back1.raw");
    assert_eq!(tool(&["export", p(&copy1), p(&back1)]).0, 0);
    assert_eq!(
        std::fs::read(&back1).expect("read raw"),
        snapshot_model(&payload)
    );
}

#[test]
fn salvage_reports_a_lost_block_and_copies_it_only_when_asked() {
    let dir = Dir::new();
    let payload = SplitMix64(1).bytes(2 * BS);
    let (hb, bytes) = base_image(1, &payload);
    let view = od_storage::cow_proto::CowView::salvage(st(&hb))
        .expect("view")
        .0;
    let head0 = view.block_offset(2).expect("head block 0");
    drop(view);
    let mut damaged = bytes.clone();
    damaged[head0 as usize + 7] ^= 0x10;
    let img = dir.file("bad.img", &damaged);
    let (copy, rep) = (dir.0.join("copy.img"), dir.0.join("report.txt"));
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy), p(&rep)]);
    assert_eq!(code, 2, "{text}");
    for want in [
        "outcome=corrupt\n",
        "recoverability=partial\n",
        "reason=block_crc_mismatch\n",
        "lost_bytes=512\n",
        "valid_bytes=512\n",
        "copy=not_created:partial_not_accepted\n",
        "extent=0+512:lost:block_crc_mismatch\n",
        "extent=512+512:valid:crc_verified\n",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(!copy.exists(), "no target without --accept-partial");
    assert!(
        std::fs::read_to_string(&rep)
            .expect("report")
            .contains("extent=0+512:lost:block_crc_mismatch\n")
    );
    assert_eq!(
        std::fs::read(&img).expect("read"),
        damaged,
        "source changed"
    );

    if !durable_host() {
        return;
    }
    let (copy2, rep2) = (dir.0.join("copy2.img"), dir.0.join("report2.txt"));
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy2), p(&rep2), "--accept-partial"]);
    assert_eq!(code, 2, "partial is never exit 0: {text}");
    assert!(text.contains("copy=published\n"), "{text}");
    let back = dir.0.join("back.raw");
    assert_eq!(tool(&["export", p(&copy2), p(&back)]).0, 0);
    let got = std::fs::read(&back).expect("read raw");
    let want = head_model(&payload);
    assert!(got[..BS].iter().all(|b| *b == 0), "the lost block is zero");
    assert_ne!(&got[..BS], &want[..BS]);
    assert_eq!(&got[BS..], &want[BS..], "verified bytes are copied");
}

#[test]
fn salvage_of_unreadable_metadata_is_unrecoverable_and_creates_no_image() {
    let dir = Dir::new();
    let (hb, bytes) = base_image(1, &SplitMix64(1).bytes(2 * BS));
    let view = od_storage::cow_proto::CowView::salvage(st(&hb))
        .expect("view")
        .0;
    let map_at = view.map_offset();
    drop(view);
    let mut damaged = bytes.clone();
    damaged[map_at as usize + 1] ^= 0x01;
    let img = dir.file("map.img", &damaged);
    let (copy, rep) = (dir.0.join("copy.img"), dir.0.join("report.txt"));
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy), p(&rep), "--accept-partial"]);
    assert_eq!(code, 2, "{text}");
    for want in [
        "recoverability=unrecoverable\n",
        "reason=map_crc_mismatch\n",
        "valid_bytes=0\n",
        "copy=not_created:unrecoverable\n",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(!copy.exists(), "no image with zero data may be produced");
    let report = std::fs::read_to_string(&rep).expect("report file");
    assert!(report.contains("finding=map:map_crc_mismatch:"), "{report}");
    assert_eq!(std::fs::read(&img).expect("read"), damaged);

    // Not an image of this format at all.
    let junk = dir.file("junk.img", &[0xEE; 4096]);
    let (copy, rep) = (dir.0.join("c2.img"), dir.0.join("r2.txt"));
    let (code, text, _) = tool(&["salvage", p(&junk), p(&copy), p(&rep)]);
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("reason=no_valid_root\n"), "{text}");
    assert!(!copy.exists() && rep.exists());

    // An unsupported mandatory feature is unsupported, not corrupt.
    let mut future = bytes.clone();
    for slot in 0..2 {
        let at = slot * 512;
        future[at + 52..at + 60].copy_from_slice(&(1u64 << 40).to_le_bytes());
        let crc = crc32(&future[at..at + 508]);
        future[at + 508..at + 512].copy_from_slice(&crc.to_le_bytes());
    }
    let img = dir.file("future.img", &future);
    let (copy, rep) = (dir.0.join("c3.img"), dir.0.join("r3.txt"));
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy), p(&rep)]);
    assert_eq!(code, 3, "{text}");
    assert!(!copy.exists());
}

#[test]
fn salvage_never_replaces_existing_files_and_respects_a_writer() {
    let dir = Dir::new();
    let (_, bytes) = base_image(1, &SplitMix64(1).bytes(BS));
    let img = dir.file("a.img", &bytes);
    let old_copy = dir.file("copy.img", b"existing image bytes");
    let rep = dir.0.join("report.txt");
    let (code, text, _) = tool(&["salvage", p(&img), p(&old_copy), p(&rep)]);
    assert_eq!(code, 5, "{text}");
    assert!(text.contains("reason=target_exists\n"), "{text}");
    assert_eq!(
        std::fs::read(&old_copy).expect("read"),
        b"existing image bytes"
    );
    assert!(!rep.exists(), "nothing may be written when a name is taken");

    let copy = dir.0.join("fresh.img");
    let old_rep = dir.file("old-report.txt", b"old report");
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy), p(&old_rep)]);
    assert_eq!(code, 5, "{text}");
    assert!(text.contains("reason=report_exists\n"), "{text}");
    assert_eq!(std::fs::read(&old_rep).expect("read"), b"old report");
    assert!(!copy.exists());

    let writer = HostFileStorage::open_read_write_exclusive(&img).expect("writer");
    let (copy, rep) = (dir.0.join("busy.img"), dir.0.join("busy.txt"));
    let (code, text, _) = tool(&["salvage", p(&img), p(&copy), p(&rep)]);
    assert_eq!(code, 4, "{text}");
    assert!(!copy.exists() && !rep.exists());
    drop(writer);
    assert_eq!(std::fs::read(&img).expect("read"), bytes);
}

#[test]
fn export_and_salvage_usage_errors_exit_one() {
    let dir = Dir::new();
    let (_, bytes) = base_image(1, &SplitMix64(1).bytes(BS));
    let path = dir.file("a.img", &bytes);
    let a = p(&path);
    let cases: Vec<Vec<&str>> = vec![
        vec!["export"],
        vec!["export", a],
        vec!["export", a, "/tmp/x.raw", "extra"],
        vec!["export", a, "/tmp/x.raw", "--snapshot"],
        vec!["export", a, "/tmp/x.raw", "--snapshot", "0"],
        vec!["export", a, "/tmp/x.raw", "--snapshot", "x"],
        vec![
            "export",
            a,
            "/tmp/x.raw",
            "--snapshot",
            "1",
            "--snapshot",
            "2",
        ],
        vec!["export", a, "/tmp/x.raw", "--accept-partial"],
        vec!["salvage", a, "/tmp/x.img"],
        vec!["salvage", a, "/tmp/x.img", "/tmp/r.txt", "extra"],
        vec!["salvage", a, "/tmp/x.img", "/tmp/r.txt", "--wat"],
        vec!["info", a, "--snapshot", "1"],
        vec!["check", a, "--accept-partial"],
    ];
    for args in cases {
        let (code, out, err) = tool(&args);
        assert_eq!(code, 1, "{args:?}: {out}{err}");
        assert!(out.is_empty() && err.contains("usage:"), "{args:?}");
    }
    assert_eq!(names(&dir), ["a.img"], "usage errors must create nothing");
}
