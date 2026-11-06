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
