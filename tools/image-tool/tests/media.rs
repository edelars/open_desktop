//! `make-data-disk` (S8-27) and `add-install-file` (S8-22): the disks are read
//! back through the storage crate's own GPT/FAT32 reader, and on macOS also
//! attached read-only with `hdiutil`.

use od_host::storage_io::ReadLease;
use od_image_tool::{Exit, Run, run};
use od_storage::install_media::{DiskListing, list_disk};
use od_storage::io::CheckedStorage;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A valid `--nonce`: exactly 32 lowercase hex digits.
const NONCE: &str = "0123456789abcdef0123456789abcdef";

const BASIC_DATA: [u8; 16] = [
    0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7,
];
const ESP: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "od-image-tool-media-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("temp dir");
        Self(path)
    }

    fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("write");
        path
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tool(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Run {
    let args: Vec<OsString> = args.iter().map(|a| a.as_ref().to_os_string()).collect();
    run(&args)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn field<'a>(run: &'a Run, key: &str) -> &'a str {
    run.stdout
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("no {key} in {}", run.stdout))
}

fn listing(path: &Path) -> (DiskListing, CheckedStorage<ReadLease>) {
    let lease = ReadLease::open(path).expect("lease");
    let source = CheckedStorage::new(lease).expect("checked");
    let length = source.size().expect("size");
    let listing = list_disk(&source, length).expect("the disk reads back through GPT and FAT32");
    (listing, source)
}

fn content(listing: &DiskListing, source: &CheckedStorage<ReadLease>, path: &str) -> Vec<u8> {
    listing
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} is not on the disk"))
        .read_to_vec(source)
        .expect("read file")
}

fn script(n: usize) -> Vec<u8> {
    (0..5000 + n * 777).map(|i| (i * 31 + n) as u8).collect()
}

/// Manifest over two scripts and an empty file, with the right hashes.
fn manifest(dir: &Dir, spare_mib: u64) -> (PathBuf, Vec<Vec<u8>>) {
    let files = [script(1), script(2), Vec::new()];
    let rows: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let name = format!("input{i}.ps1");
            dir.put(&name, bytes);
            format!(
                r#"{{"path": "OD8\\{}", "source": "{name}", "sha256": "{}", "origin": "test"}}"#,
                if i == 2 {
                    "sub/empty.ps1".to_owned()
                } else {
                    name.clone()
                },
                sha256(bytes)
            )
        })
        .collect();
    let text = format!(
        r#"{{"version": 1, "comment": "test", "spare_mib": {spare_mib}, "files": [{}]}}"#,
        rows.join(",")
    );
    (dir.put("manifest.json", text.as_bytes()), files.to_vec())
}

fn no_output(dir: &Dir, name: &str) {
    assert!(!dir.path(name).exists(), "{name} must not exist");
    assert!(
        !dir.path(&format!("{name}.partial")).exists(),
        "{name}.partial must not exist"
    );
}

#[test]
fn data_disk_reads_back_with_the_expected_layout() {
    let dir = Dir::new();
    let (manifest, files) = manifest(&dir, 1);
    let out = dir.path("payload.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out, &"--nonce", &NONCE]);
    assert_eq!(run.exit, Exit::Ok, "{}{}", run.stdout, run.stderr);
    assert_eq!(field(&run, "volume_label"), "OD8PAYLOAD");
    assert_eq!(field(&run, "published"), "true");
    assert_eq!(field(&run, "nonce_present"), "true");
    let bytes = std::fs::read(&out).expect("output");
    assert_eq!(field(&run, "output_sha256"), sha256(&bytes));
    assert_eq!(field(&run, "logical_size"), bytes.len().to_string());
    assert!(!dir.path("payload.raw.partial").exists());

    let (listing, source) = listing(&out);
    assert_eq!(listing.label, "OD8PAYLOAD");
    assert_eq!(listing.partition_type, BASIC_DATA, "Basic Data, not ESP");
    assert_ne!(listing.partition_type, ESP);
    assert_eq!(listing.directories, ["OD8", "OD8/sub"]);
    assert_eq!(
        content(&listing, &source, "OD8/nonce.txt"),
        NONCE.as_bytes()
    );
    assert_eq!(content(&listing, &source, "OD8/markers.txt"), b"");
    assert_eq!(content(&listing, &source, "OD8/input0.ps1"), files[0]);
    assert_eq!(content(&listing, &source, "OD8/input1.ps1"), files[1]);
    assert_eq!(content(&listing, &source, "OD8/sub/empty.ps1"), b"");
    assert_eq!(listing.files.len(), 5);
    // Free space is left for the guest: at least the requested spare.
    assert!(bytes.len() > 32 * 1024 * 1024);
}

/// The checked-in example manifest must keep matching the files it names.
#[test]
fn checked_in_example_manifest_builds() {
    let dir = Dir::new();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/compat/payloads/stage0-fixtures.example.json");
    let out = dir.path("example.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out, &"--nonce", &NONCE]);
    assert_eq!(run.exit, Exit::Ok, "{}", run.stdout);
    assert_eq!(field(&run, "input_files"), "3");
    let (listing, source) = listing(&out);
    assert_eq!(listing.partition_type, BASIC_DATA);
    let script = content(&listing, &source, "OD8/stage0/node_workload.mjs");
    assert_eq!(
        sha256(&script),
        "5610cb9abd28ba9bd357e4e9c6359133184c5d11b8fb0146cf8654c210e8c335"
    );
}

#[test]
fn data_disk_without_nonce_has_an_empty_nonce_file() {
    let dir = Dir::new();
    let (manifest, _) = manifest(&dir, 1);
    let out = dir.path("payload.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out]);
    assert_eq!(run.exit, Exit::Ok, "{}", run.stdout);
    assert_eq!(field(&run, "nonce_present"), "false");
    let (listing, source) = listing(&out);
    assert_eq!(content(&listing, &source, "OD8/nonce.txt"), b"");
}

#[test]
fn data_disk_with_a_600_mib_spare_gives_a_large_volume() {
    let dir = Dir::new();
    let (manifest, files) = manifest(&dir, 600);
    let out = dir.path("payload.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out]);
    assert_eq!(run.exit, Exit::Ok, "{}", run.stdout);
    let size: u64 = field(&run, "logical_size").parse().expect("number");
    assert!(size > 600 * 1024 * 1024);
    let (listing, source) = listing(&out);
    assert_eq!(content(&listing, &source, "OD8/input1.ps1"), files[1]);
}

#[test]
fn hash_mismatch_refuses_without_an_output_file() {
    let dir = Dir::new();
    let (manifest, _) = manifest(&dir, 1);
    // The second input changed after the manifest was written.
    dir.put("input1.ps1", b"tampered");
    let out = dir.path("payload.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out]);
    assert_eq!(run.exit, Exit::Corrupt, "{}", run.stdout);
    assert_eq!(field(&run, "reason"), "input_hash_mismatch");
    assert_eq!(field(&run, "detail"), "1");
    assert_eq!(field(&run, "actual_sha256"), sha256(b"tampered"));
    assert_eq!(field(&run, "published"), "false");
    assert_eq!(field(&run, "partial_left"), "false");
    no_output(&dir, "payload.raw");
}

#[test]
fn existing_output_is_refused_and_left_alone() {
    let dir = Dir::new();
    let (manifest, _) = manifest(&dir, 1);
    let out = dir.put("payload.raw", b"keep me");
    let run = tool(&[&"make-data-disk", &manifest, &out]);
    assert_eq!(run.exit, Exit::Io, "{}", run.stdout);
    assert_eq!(field(&run, "reason"), "target_exists");
    assert_eq!(std::fs::read(&out).expect("read"), b"keep me");
    assert!(!dir.path("payload.raw.partial").exists());

    // A second run over a disk the tool made is refused the same way.
    let fresh = dir.path("second.raw");
    assert_eq!(tool(&[&"make-data-disk", &manifest, &fresh]).exit, Exit::Ok);
    let before = std::fs::read(&fresh).expect("read");
    let again = tool(&[&"make-data-disk", &manifest, &fresh]);
    assert_eq!(again.exit, Exit::Io);
    assert_eq!(field(&again, "reason"), "target_exists");
    assert_eq!(std::fs::read(&fresh).expect("read"), before);

    // A leftover .partial is refused as well.
    dir.put("third.raw.partial", b"x");
    let third = tool(&[&"make-data-disk", &manifest, &dir.path("third.raw")]);
    assert_eq!(third.exit, Exit::Io);
    assert_eq!(field(&third, "reason"), "partial_exists");
    assert!(!dir.path("third.raw").exists());
}

#[test]
fn bad_manifests_and_arguments_create_nothing() {
    let dir = Dir::new();
    let good = dir.put("good.bin", b"data");
    let hash = sha256(b"data");
    let cases: [(&str, String, &str); 7] = [
        ("not json", "{".to_owned(), "manifest_json"),
        (
            "unknown key",
            format!(r#"{{"version":1,"files":[],"extra":1}}"#),
            "manifest_field",
        ),
        (
            "wrong version",
            r#"{"version":2,"files":[]}"#.to_owned(),
            "manifest_field",
        ),
        (
            "outside OD8",
            format!(
                r#"{{"version":1,"files":[{{"path":"other/a.bin","source":"good.bin","sha256":"{hash}"}}]}}"#
            ),
            "manifest_path",
        ),
        (
            "bad sha",
            r#"{"version":1,"files":[{"path":"OD8/a.bin","source":"good.bin","sha256":"zz"}]}"#
                .to_owned(),
            "manifest_sha256",
        ),
        (
            "reserved name",
            format!(
                r#"{{"version":1,"files":[{{"path":"OD8/NONCE.TXT","source":"good.bin","sha256":"{hash}"}}]}}"#
            ),
            "media_file_exists",
        ),
        (
            "bad spare",
            r#"{"version":1,"spare_mib":0,"files":[]}"#.to_owned(),
            "manifest_spare",
        ),
    ];
    assert!(good.exists());
    for (index, (name, text, reason)) in cases.iter().enumerate() {
        let manifest = dir.put("m.json", text.as_bytes());
        let out = format!("out{index}.raw");
        let run = tool(&[&"make-data-disk", &manifest, &dir.path(&out)]);
        assert_eq!(run.exit, Exit::Unsupported, "{name}: {}", run.stdout);
        assert_eq!(field(&run, "reason"), *reason, "{name}");
        no_output(&dir, &out);
    }
    let (manifest, _) = self::manifest(&dir, 1);
    let run = tool(&[
        &"make-data-disk",
        &manifest,
        &dir.path("n.raw"),
        &"--nonce",
        &"bad nonce",
    ]);
    assert_eq!(run.exit, Exit::Unsupported);
    assert_eq!(field(&run, "reason"), "nonce_invalid");
    no_output(&dir, "n.raw");
    let run = tool(&[
        &"make-data-disk",
        &dir.path("missing.json"),
        &dir.path("m.raw"),
    ]);
    assert_eq!(run.exit, Exit::Io);
    no_output(&dir, "m.raw");
    assert_eq!(tool(&[&"make-data-disk", &manifest]).exit, Exit::Usage);
}

#[test]
fn missing_input_file_refuses_without_an_output_file() {
    let dir = Dir::new();
    let (manifest, _) = manifest(&dir, 1);
    std::fs::remove_file(dir.path("input0.ps1")).expect("remove");
    let run = tool(&[&"make-data-disk", &manifest, &dir.path("p.raw")]);
    assert_eq!(run.exit, Exit::Io, "{}", run.stdout);
    assert_eq!(field(&run, "reason"), "not_found");
    assert_eq!(field(&run, "detail"), "0");
    no_output(&dir, "p.raw");
}

/// A small ISO 9660 volume: `SOURCES\SETUP.EXE` and an empty `SOURCES` otherwise.
fn tiny_iso(setup: &[u8]) -> Vec<u8> {
    fn both32(value: u32) -> [u8; 8] {
        let mut out = [0; 8];
        out[..4].copy_from_slice(&value.to_le_bytes());
        out[4..].copy_from_slice(&value.to_be_bytes());
        out
    }
    fn both16(value: u16) -> [u8; 4] {
        let mut out = [0; 4];
        out[..2].copy_from_slice(&value.to_le_bytes());
        out[2..].copy_from_slice(&value.to_be_bytes());
        out
    }
    fn record(lba: u32, length: u32, directory: bool, name: &[u8]) -> Vec<u8> {
        let mut row = vec![0; 33];
        row[2..10].copy_from_slice(&both32(lba));
        row[10..18].copy_from_slice(&both32(length));
        row[25] = if directory { 2 } else { 0 };
        row[28..32].copy_from_slice(&both16(1));
        row[32] = name.len() as u8;
        row.extend_from_slice(name);
        if name.len() % 2 == 0 {
            row.push(0);
        }
        row[0] = row.len() as u8;
        row
    }
    let blocks = 20 + setup.len().div_ceil(2048);
    let mut iso = vec![0u8; blocks * 2048];
    let mut put = |sector: usize, rows: &[Vec<u8>]| {
        let mut at = sector * 2048;
        for row in rows {
            iso[at..at + row.len()].copy_from_slice(row);
            at += row.len();
        }
    };
    let root = record(18, 2048, true, &[0]);
    let mut pvd = vec![0u8; 2048];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[80..88].copy_from_slice(&both32(blocks as u32));
    pvd[120..124].copy_from_slice(&both16(1));
    pvd[124..128].copy_from_slice(&both16(1));
    pvd[128..132].copy_from_slice(&both16(2048));
    pvd[156..156 + root.len()].copy_from_slice(&root);
    pvd[881] = 1;
    let mut end = vec![0u8; 7];
    end[0] = 255;
    end[1..6].copy_from_slice(b"CD001");
    end[6] = 1;
    put(16, &[pvd]);
    put(17, &[end]);
    put(
        18,
        &[
            root.clone(),
            record(18, 2048, true, &[1]),
            record(19, 2048, true, b"SOURCES"),
        ],
    );
    put(
        19,
        &[
            record(19, 2048, true, &[0]),
            record(18, 2048, true, &[1]),
            record(20, setup.len() as u32, false, b"SETUP.EXE;1"),
        ],
    );
    iso[20 * 2048..20 * 2048 + setup.len()].copy_from_slice(setup);
    iso
}

#[test]
fn answer_file_is_added_to_a_copy_of_the_converted_media() {
    let dir = Dir::new();
    let setup = script(9);
    let iso = dir.put("media.iso", &tiny_iso(&setup));
    let converted = dir.path("media.raw");
    let converted_run = tool(&[&"import-install-media", &iso, &converted]);
    assert_eq!(converted_run.exit, Exit::Ok, "{}", converted_run.stdout);
    let original = std::fs::read(&converted).expect("converted");
    let answer = b"<?xml version=\"1.0\"?><unattend/>".to_vec();
    let answer_path = dir.put("Autounattend.xml", &answer);

    let out = dir.path("with-answer.raw");
    let run = tool(&[
        &"add-install-file",
        &converted,
        &out,
        &"sources\\Autounattend.xml",
        &answer_path,
    ]);
    assert_eq!(run.exit, Exit::Ok, "{}{}", run.stdout, run.stderr);
    assert_eq!(field(&run, "input_sha256"), sha256(&original));
    assert_eq!(field(&run, "added_file_sha256"), sha256(&answer));
    assert_eq!(field(&run, "input_unchanged"), "true");
    let result = std::fs::read(&out).expect("output");
    assert_eq!(field(&run, "output_sha256"), sha256(&result));
    assert_ne!(sha256(&result), sha256(&original));
    // The source medium is untouched.
    assert_eq!(std::fs::read(&converted).expect("converted"), original);
    assert!(!dir.path("with-answer.raw.partial").exists());

    let (listing, source) = listing(&out);
    assert_eq!(listing.label, "OD INSTALL");
    assert_eq!(listing.partition_type, ESP);
    assert_eq!(
        content(&listing, &source, "SOURCES/Autounattend.xml"),
        answer
    );
    assert_eq!(content(&listing, &source, "SOURCES/SETUP.EXE"), setup);
    let (before, _) = listing_of(&converted);
    assert_eq!(before.files.len() + 1, listing.files.len());

    // Adding again to the same name, or over an existing file, or into a
    // missing directory is refused and creates nothing.
    let again = tool(&[
        &"add-install-file",
        &converted,
        &out,
        &"sources/x.xml",
        &answer_path,
    ]);
    assert_eq!(again.exit, Exit::Io);
    assert_eq!(field(&again, "reason"), "target_exists");
    for (path, reason) in [
        ("sources/setup.exe", "media_file_exists"),
        ("nowhere/a.xml", "media_directory_missing"),
        ("sources/bad:name", "media_path"),
    ] {
        let name = format!("refused-{reason}.raw");
        let run = tool(&[
            &"add-install-file",
            &converted,
            &dir.path(&name),
            &path,
            &answer_path,
        ]);
        assert_eq!(run.exit, Exit::Unsupported, "{path}: {}", run.stdout);
        assert_eq!(field(&run, "reason"), reason);
        no_output(&dir, &name);
    }
    // A disk that is not a converted medium (the payload disk is Basic Data) is refused.
    let (manifest, _) = manifest(&dir, 1);
    let payload = dir.path("payload.raw");
    assert_eq!(
        tool(&[&"make-data-disk", &manifest, &payload]).exit,
        Exit::Ok
    );
    let run = tool(&[
        &"add-install-file",
        &payload,
        &dir.path("x.raw"),
        &"OD8/a.xml",
        &answer_path,
    ]);
    assert_eq!(run.exit, Exit::Unsupported, "{}", run.stdout);
    assert_eq!(field(&run, "reason"), "media_partition_type");
    no_output(&dir, "x.raw");
}

fn listing_of(path: &Path) -> (DiskListing, CheckedStorage<ReadLease>) {
    listing(path)
}

/// Manifest with one real file at `path` (as written in the manifest) and no
/// `spare_mib` unless `spare` is given.
fn single_file_manifest(dir: &Dir, path: &str, spare: Option<u64>) -> PathBuf {
    let bytes = b"data";
    dir.put("good.bin", bytes);
    let mut manifest = serde_json::json!({"version": 1, "files": [
        {"path": path, "source": "good.bin", "sha256": sha256(bytes)}
    ]});
    if let Some(mib) = spare {
        manifest["spare_mib"] = mib.into();
    }
    dir.put("single.json", manifest.to_string().as_bytes())
}

#[test]
fn data_disk_without_spare_in_the_manifest_leaves_512_mib() {
    let dir = Dir::new();
    let manifest = single_file_manifest(&dir, "OD8/a.bin", None);
    let out = dir.path("payload.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out]);
    assert_eq!(run.exit, Exit::Ok, "{}{}", run.stdout, run.stderr);
    let size: u64 = field(&run, "logical_size").parse().expect("number");
    let mib = 1024 * 1024;
    assert!(
        (512 * mib..520 * mib).contains(&size),
        "the default spare is 512 MiB, got a disk of {size} bytes"
    );
    let (listing, source) = listing(&out);
    assert_eq!(content(&listing, &source, "OD8/a.bin"), b"data");
}

#[test]
fn spare_above_what_the_reader_accepts_is_refused_before_anything_is_written() {
    let dir = Dir::new();
    // The reader accepts a FAT of 64 MiB: 16_777_214 clusters of 4 KiB, so
    // 65536 MiB of spare (16_777_216 clusters) never fits and is refused as
    // a manifest error, whatever the payload.
    for (index, spare) in [64 * 1024, 64 * 1024 + 1].into_iter().enumerate() {
        let manifest = single_file_manifest(&dir, "OD8/a.bin", Some(spare));
        let out = format!("big{index}.raw");
        let run = tool(&[&"make-data-disk", &manifest, &dir.path(&out)]);
        assert_eq!(run.exit, Exit::Unsupported, "{}", run.stdout);
        assert_eq!(field(&run, "reason"), "manifest_spare", "{spare}");
        no_output(&dir, &out);
    }

    // 65535 MiB is the last value that is not refused outright, but a payload
    // of 1.5 MiB (384 clusters) next to it is over the FAT limit.
    let big = vec![7u8; 1536 * 1024];
    dir.put("big.bin", &big);
    let manifest = serde_json::json!({"version": 1, "spare_mib": 64 * 1024 - 1, "files": [
        {"path": "OD8/big.bin", "source": "big.bin", "sha256": sha256(&big)}
    ]});
    let manifest = dir.put("big.json", manifest.to_string().as_bytes());
    let run = tool(&[&"make-data-disk", &manifest, &dir.path("edge.raw")]);
    assert_eq!(run.exit, Exit::Unsupported, "{}", run.stdout);
    assert_eq!(field(&run, "reason"), "fat_capacity");
    assert_eq!(field(&run, "published"), "false");
    assert_eq!(field(&run, "partial_left"), "false");
    no_output(&dir, "edge.raw");
}

/// `OD8` counts as the first directory; the reader accepts 16 of them.
fn nested_path(subdirectories: usize) -> String {
    let mut path = String::from("OD8");
    for index in 1..=subdirectories {
        path.push_str(&format!("/d{index}"));
    }
    path.push_str("/f.bin");
    path
}

#[test]
fn directory_depth_at_the_reader_limit_reads_back_and_one_more_is_refused() {
    let dir = Dir::new();
    // OD8 and 15 subdirectories: 16 levels, the most the reader takes.
    let deepest = nested_path(15);
    let manifest = single_file_manifest(&dir, &deepest, Some(1));
    let out = dir.path("deep.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out]);
    assert_eq!(run.exit, Exit::Ok, "{}{}", run.stdout, run.stderr);
    let (listing, source) = listing(&out);
    assert_eq!(content(&listing, &source, &deepest), b"data");

    // 17 and 18 levels (the second is 18 nested directories) are refused
    // by the manifest before any file is made.
    for (index, subdirectories) in [16, 17].into_iter().enumerate() {
        let manifest = single_file_manifest(&dir, &nested_path(subdirectories), Some(1));
        let out = format!("toodeep{index}.raw");
        let run = tool(&[&"make-data-disk", &manifest, &dir.path(&out)]);
        assert_eq!(run.exit, Exit::Unsupported, "{}", run.stdout);
        assert_eq!(field(&run, "reason"), "manifest_depth", "{subdirectories}");
        assert_eq!(field(&run, "detail"), "0");
        assert_eq!(field(&run, "published"), "false");
        assert_eq!(field(&run, "partial_left"), "false");
        no_output(&dir, &out);
    }
}

#[test]
fn nonce_must_be_32_lowercase_hex_digits() {
    let dir = Dir::new();
    let (manifest, _) = manifest(&dir, 1);
    let cases = [
        ("empty", String::new()),
        ("uppercase", NONCE.to_uppercase()),
        ("31 digits", NONCE[1..].to_owned()),
        ("33 digits", format!("{NONCE}0")),
        ("not hex", "g".repeat(32)),
        ("trailing newline", format!("{}\n", &NONCE[1..])),
        ("old format", "abc-123.X".to_owned()),
        ("64 digits", NONCE.repeat(2)),
    ];
    for (index, (name, nonce)) in cases.iter().enumerate() {
        let out = format!("nonce{index}.raw");
        let run = tool(&[
            &"make-data-disk",
            &manifest,
            &dir.path(&out),
            &"--nonce",
            nonce,
        ]);
        assert_eq!(run.exit, Exit::Unsupported, "{name}: {}", run.stdout);
        assert_eq!(field(&run, "reason"), "nonce_invalid", "{name}");
        no_output(&dir, &out);
    }
    // The valid value is accepted and is the content of the file.
    let out = dir.path("ok.raw");
    let run = tool(&[&"make-data-disk", &manifest, &out, &"--nonce", &NONCE]);
    assert_eq!(run.exit, Exit::Ok, "{}", run.stdout);
    let (listing, source) = listing(&out);
    assert_eq!(
        content(&listing, &source, "OD8/nonce.txt"),
        NONCE.as_bytes()
    );
}

#[test]
fn manifest_paths_that_leave_od8_are_refused_and_create_nothing() {
    let dir = Dir::new();
    let cases = [
        ("../x", "manifest_path"),
        ("/x", "manifest_path"),
        ("/OD8/x", "manifest_path"),
        ("C:/x", "manifest_path"),
        ("C:\\OD8\\x", "manifest_path"),
        ("od8/x", "manifest_path"),
        ("OD8", "manifest_path"),
        ("OD8/", "manifest_path"),
        ("OD8//x", "media_path"),
        ("OD8/./x", "media_path"),
        ("OD8/../x", "media_path"),
        ("OD8\\..\\x", "media_path"),
        ("OD8/a/../../x", "media_path"),
    ];
    for (index, (path, reason)) in cases.iter().enumerate() {
        let manifest = single_file_manifest(&dir, path, Some(1));
        let out = format!("path{index}.raw");
        let run = tool(&[&"make-data-disk", &manifest, &dir.path(&out)]);
        assert_eq!(run.exit, Exit::Unsupported, "{path}: {}", run.stdout);
        assert_eq!(field(&run, "reason"), *reason, "{path}");
        no_output(&dir, &out);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_attaches_the_data_disk_read_only() {
    use std::process::Command;
    let dir = Dir::new();
    let (manifest, files) = manifest(&dir, 1);
    let out = dir.path("payload.raw");
    assert_eq!(
        tool(&[&"make-data-disk", &manifest, &out, &"--nonce", &NONCE]).exit,
        Exit::Ok
    );
    let mount = dir.path("mnt");
    std::fs::create_dir(&mount).expect("mount point");
    let attach = Command::new("hdiutil")
        .args(["attach", "-readonly", "-nobrowse", "-noverify", "-plist"])
        .args(["-imagekey", "diskimage-class=CRawDiskImage", "-mountpoint"])
        .arg(&mount)
        .arg(&out)
        .output();
    // Only a missing hdiutil skips the test; a failed attach is a failure.
    let attach = match attach {
        Ok(output) => output,
        Err(error) => {
            eprintln!("hdiutil cannot be started, skipped: {error}");
            return;
        }
    };
    assert!(
        attach.status.success(),
        "hdiutil attach failed with {}: {}{}",
        attach.status,
        String::from_utf8_lossy(&attach.stdout),
        String::from_utf8_lossy(&attach.stderr)
    );
    let text = String::from_utf8_lossy(&attach.stdout).into_owned();
    // Detach on every path out of the checks below.
    struct Detach(PathBuf);
    impl Drop for Detach {
        fn drop(&mut self) {
            let _ = Command::new("hdiutil")
                .arg("detach")
                .arg(&self.0)
                .arg("-force")
                .output();
        }
    }
    let _detach = Detach(mount.clone());
    assert!(
        text.contains("OD8PAYLOAD") || mount.join("OD8").exists(),
        "{text}"
    );
    assert_eq!(
        std::fs::read(mount.join("OD8/nonce.txt")).expect("nonce"),
        NONCE.as_bytes()
    );
    assert_eq!(
        std::fs::read(mount.join("OD8/markers.txt")).expect("markers"),
        b""
    );
    assert_eq!(
        std::fs::read(mount.join("OD8/input0.ps1")).expect("script"),
        files[0]
    );
    assert_eq!(
        std::fs::read(mount.join("OD8/input1.ps1")).expect("script"),
        files[1]
    );
    assert!(
        std::fs::write(mount.join("OD8/new.txt"), b"x").is_err(),
        "mounted read-only"
    );
}
