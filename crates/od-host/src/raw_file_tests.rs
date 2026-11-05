//! RAW image on real host files (S9-05). The image code lives in `od-storage`
//! and cannot name `od-host`, so the file-backed checks run here. Every
//! expectation about disk contents is taken from `std::fs::read`, never from
//! the image reading its own bytes back.

use crate::storage_io::{HostFileStorage, WriterOpenError, process_spawn_guard};
use od_storage::io::StorageError;
use od_storage::raw::{RawError, RawImage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const BS: u64 = 512;
const SIZE: usize = 8192;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "od-host-raw-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn create(path: &Path, size: usize) -> RawImage<HostFileStorage> {
    let backend = HostFileStorage::create_new_exclusive(path, size as u64).expect("create file");
    RawImage::create(backend, size as u64, BS).expect("create raw image")
}

fn open_rw(path: &Path) -> RawImage<HostFileStorage> {
    let backend = HostFileStorage::open_read_write_exclusive(path).expect("open exclusive");
    RawImage::open_read_write(backend, Some(SIZE as u64), BS).expect("open raw image")
}

fn open_ro(path: &Path) -> RawImage<HostFileStorage> {
    let backend = HostFileStorage::open_read_only(path).expect("open read-only");
    RawImage::open_read_only(backend, Some(SIZE as u64), BS).expect("open raw image")
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed))
        .collect()
}

fn apply(reference: &mut [u8], offset: usize, data: &[u8]) {
    reference[offset..offset + data.len()].copy_from_slice(data);
}

#[test]
fn create_makes_file_of_exact_size_full_of_zeros() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("zeros.raw");
    let image = create(&path, SIZE);
    assert_eq!(image.logical_size(), SIZE as u64);
    assert_eq!(std::fs::read(&path).unwrap(), vec![0u8; SIZE]);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), SIZE as u64);
}

#[test]
fn writes_reopen_and_independent_comparison() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("disk.raw");
    let mut reference = vec![0u8; SIZE];

    let mut image = create(&path, SIZE);
    // Start, end, middle, unaligned inside a block, across block boundaries,
    // and aligned whole blocks.
    let head = pattern(40, 1);
    let tail = pattern(40, 2);
    let middle = pattern(1500, 3);
    let odd = pattern(11, 4);
    let straddle = pattern(9, 5);
    let block = pattern(BS as usize, 6);
    image.write_at(0, &head).unwrap();
    apply(&mut reference, 0, &head);
    image.write_at((SIZE - 40) as u64, &tail).unwrap();
    apply(&mut reference, SIZE - 40, &tail);
    image.write_at(3001, &middle).unwrap();
    apply(&mut reference, 3001, &middle);
    image.write_at(517, &odd).unwrap();
    apply(&mut reference, 517, &odd);
    image.write_at(BS * 6 - 4, &straddle).unwrap();
    apply(&mut reference, BS as usize * 6 - 4, &straddle);
    image.write_aligned(BS * 2, &block).unwrap();
    apply(&mut reference, BS as usize * 2, &block);
    // Independent observation while the writer is still open.
    assert_eq!(std::fs::read(&path).unwrap(), reference);
    drop(image);

    // Reopen as writer: contents survive, then a further write is visible.
    let mut image = open_rw(&path);
    let mut seen = vec![0u8; SIZE];
    image.read_at(0, &mut seen).unwrap();
    assert_eq!(seen, reference);
    let more = pattern(23, 9);
    image.write_at(4099, &more).unwrap();
    apply(&mut reference, 4099, &more);
    drop(image);
    assert_eq!(std::fs::read(&path).unwrap(), reference);

    // Reopen read-only: the same bytes, byte-granular and aligned.
    let image = open_ro(&path);
    let mut seen = vec![0u8; SIZE];
    image.read_at(0, &mut seen).unwrap();
    assert_eq!(seen, reference);
    let mut part = vec![0u8; BS as usize];
    image.read_aligned(BS * 2, &mut part).unwrap();
    assert_eq!(part, block);
}

#[test]
fn create_over_existing_file_refuses_and_keeps_bytes() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("existing.raw");
    let original = pattern(SIZE, 0x5A);
    std::fs::write(&path, &original).unwrap();

    // Same size, smaller and larger requests all refuse; none truncates.
    for len in [SIZE as u64, 512, 2 * SIZE as u64] {
        match HostFileStorage::create_new_exclusive(&path, len) {
            Err(WriterOpenError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists)
            }
            other => panic!("expected AlreadyExists, got {:?}", other.map(|_| ())),
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    // The same holds after a writable image was opened on that file.
    let mut image = open_rw(&path);
    image.write_at(10, &[1, 2, 3]).unwrap();
    drop(image);
    let mut expected = original.clone();
    apply(&mut expected, 10, &[1, 2, 3]);
    assert!(HostFileStorage::create_new_exclusive(&path, SIZE as u64).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), expected);
}

#[test]
fn range_checks_on_real_file_leave_it_unchanged() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("range.raw");
    let mut image = create(&path, SIZE);
    let size = SIZE as u64;
    let mut reference = vec![0u8; SIZE];

    // Exactly up to the end is allowed.
    let last = pattern(4, 8);
    image.write_at(size - 4, &last).unwrap();
    apply(&mut reference, SIZE - 4, &last);
    // One byte beyond is OutOfRange and nothing is written or grown.
    assert_eq!(
        image.write_at(size - 3, &last),
        Err(RawError::OutOfRange {
            offset: size - 3,
            len: 4,
            size
        })
    );
    let mut buf = [0u8; 4];
    assert_eq!(
        image.read_at(size - 3, &mut buf),
        Err(RawError::OutOfRange {
            offset: size - 3,
            len: 4,
            size
        })
    );
    assert!(matches!(
        image.write_at(size, &[1]),
        Err(RawError::OutOfRange { .. })
    ));
    assert_eq!(
        image.write_at(u64::MAX, &last),
        Err(RawError::OffsetOverflow {
            offset: u64::MAX,
            len: 4
        })
    );
    assert_eq!(std::fs::read(&path).unwrap(), reference);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
}

#[test]
fn read_only_reopen_refuses_writes() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("ro.raw");
    let mut image = create(&path, SIZE);
    let data = pattern(100, 7);
    image.write_at(200, &data).unwrap();
    drop(image);
    let before = std::fs::read(&path).unwrap();

    let mut image = open_ro(&path);
    assert_eq!(
        image.write_at(0, &[1]),
        Err(RawError::Storage(StorageError::ReadOnly))
    );
    assert_eq!(
        image.write_aligned(0, &[0; 512]),
        Err(RawError::Storage(StorageError::ReadOnly))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // A read-only handle is not a writer: it does not block a later writer.
    let writer = open_rw(&path);
    drop(writer);
    drop(image);
}

#[test]
fn second_writer_is_busy_while_image_is_open() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("busy.raw");
    let image = create(&path, SIZE);
    assert!(matches!(
        HostFileStorage::open_read_write_exclusive(&path),
        Err(WriterOpenError::WriterBusy)
    ));
    drop(image);
    drop(open_rw(&path));
}

#[test]
fn short_file_is_truncated_not_zero_padded() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("short.raw");
    let data = pattern(SIZE - 512, 3);
    std::fs::write(&path, &data).unwrap();

    let backend = HostFileStorage::open_read_write_exclusive(&path).unwrap();
    assert_eq!(
        RawImage::open_read_write(backend, Some(SIZE as u64), BS).err(),
        Some(RawError::Truncated {
            expected: SIZE as u64,
            actual: SIZE as u64 - 512
        })
    );
    let backend = HostFileStorage::open_read_only(&path).unwrap();
    assert_eq!(
        RawImage::open_read_only(backend, Some(SIZE as u64), BS).err(),
        Some(RawError::Truncated {
            expected: SIZE as u64,
            actual: SIZE as u64 - 512
        })
    );
    // The refused open neither padded nor changed the file.
    assert_eq!(std::fs::read(&path).unwrap(), data);

    // Without a metadata size the logical size is the file size, and the
    // first byte past it is OutOfRange.
    let backend = HostFileStorage::open_read_only(&path).unwrap();
    let image = RawImage::open_read_only(backend, None, BS).unwrap();
    assert_eq!(image.logical_size(), data.len() as u64);
    let mut buf = [0u8; 1];
    assert!(matches!(
        image.read_at(data.len() as u64, &mut buf),
        Err(RawError::OutOfRange { .. })
    ));
}

#[test]
fn file_truncated_behind_open_image_is_incomplete() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let path = dir.path("shrunk.raw");
    let mut image = create(&path, SIZE);
    image.write_at(0, &pattern(SIZE, 1)).unwrap();
    // Another party shrinks the file (advisory lock does not stop it).
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(1024)
        .unwrap();
    let mut buf = [0u8; 16];
    // Ends exactly at the new file end: still served.
    image.read_at(1008, &mut buf).unwrap();
    assert_eq!(
        image.read_at(1016, &mut buf),
        Err(RawError::IncompleteImage {
            offset: 1016,
            len: 16,
            logical_size: SIZE as u64
        })
    );
    assert!(matches!(
        image.write_at(2048, &buf),
        Err(RawError::IncompleteImage { .. })
    ));
    // Still inside the surviving part: fine.
    image.read_at(0, &mut buf).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 1024);
}

#[test]
fn host_image_declares_no_durability() {
    let _serial = process_spawn_guard();
    let dir = TempDir::new();
    let image = create(&dir.path("caps.raw"), SIZE);
    let caps = image.capabilities();
    assert!(caps.writable && caps.exclusive_lock);
    assert!(!caps.file_durability && !caps.namespace_durability);
    let mut storage = image.into_storage();
    assert!(matches!(
        storage.flush_file_durable(),
        Err(StorageError::UnsupportedDurability { .. })
    ));
}
