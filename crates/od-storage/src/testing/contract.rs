//! One contract suite run against every [`StorageBackend`].
//!
//! The expected bytes come from a plain `Vec<u8>` model that is updated with
//! slice operations only. It shares no code with any backend or with
//! `CheckedStorage`. Backends that can expose their bytes outside the backend
//! (for example a real file read through the filesystem) also have the final
//! content compared against the model through
//! [`ContractFixture::external_bytes`].

use crate::io::{CheckedStorage, DurabilityScope, StorageBackend, StorageError};

/// Creates fresh backends for the suite.
pub trait ContractFixture {
    type Backend: StorageBackend;

    /// A new writable object holding exactly `initial`.
    fn create(&mut self, initial: &[u8]) -> Self::Backend;

    /// A new object holding exactly `initial`, opened read-only.
    fn create_read_only(&mut self, initial: &[u8]) -> Self::Backend;

    /// Bytes of the most recently created object as observed outside the
    /// backend, or `None` when that is not possible.
    fn external_bytes(&mut self) -> Option<Vec<u8>> {
        None
    }
}

use crate::testing::rng::SplitMix64 as Gen;

fn checked<B: StorageBackend>(backend: B) -> CheckedStorage<B> {
    CheckedStorage::new(backend).expect("backend declares consistent capabilities")
}

fn read_all<B: StorageBackend>(storage: &CheckedStorage<B>) -> Vec<u8> {
    let size = storage.size().expect("size");
    let mut out = vec![0u8; size as usize];
    storage.read_exact_at(0, &mut out).expect("full read");
    out
}

/// Runs the complete contract. Panics with a descriptive message on the first
/// violation.
pub fn run_contract<F: ContractFixture>(fixture: &mut F) {
    capabilities_are_consistent(fixture);
    empty_object(fixture);
    bounds_and_overflow(fixture);
    model_based_random_io(fixture);
    large_transfer(fixture);
    resize_behavior(fixture);
    read_only_behavior(fixture);
    durability_matches_capabilities(fixture);
}

fn capabilities_are_consistent<F: ContractFixture>(fixture: &mut F) {
    let backend = fixture.create(&[1, 2, 3]);
    let caps = backend.capabilities();
    assert_eq!(caps.validate(), Ok(()));
    assert!(caps.writable, "writable fixture must declare writable");
    assert!(caps.max_len >= 3);
    let read_only = fixture.create_read_only(&[1, 2, 3]).capabilities();
    assert_eq!(read_only.validate(), Ok(()));
    assert!(!read_only.writable);
    assert!(!read_only.resize);
}

fn empty_object<F: ContractFixture>(fixture: &mut F) {
    let mut storage = checked(fixture.create(&[]));
    assert_eq!(storage.size(), Ok(0));
    storage.read_exact_at(0, &mut []).expect("empty read at 0");
    assert_eq!(
        storage.read_exact_at(0, &mut [0u8; 1]),
        Err(StorageError::OutOfRange {
            offset: 0,
            len: 1,
            size: 0
        })
    );
    assert_eq!(
        storage.write_all_at(0, &[1]),
        Err(StorageError::OutOfRange {
            offset: 0,
            len: 1,
            size: 0
        })
    );
    storage.write_all_at(0, &[]).expect("empty write at 0");
}

fn bounds_and_overflow<F: ContractFixture>(fixture: &mut F) {
    let initial = Gen(1).bytes(64);
    let mut storage = checked(fixture.create(&initial));
    assert_eq!(storage.size(), Ok(64));

    let mut buf = [0u8; 64];
    storage.read_exact_at(0, &mut buf).expect("whole object");
    assert_eq!(buf.as_slice(), initial.as_slice());
    let mut last = [0u8; 1];
    storage.read_exact_at(63, &mut last).expect("last byte");
    assert_eq!(last[0], initial[63]);
    storage
        .read_exact_at(64, &mut [])
        .expect("empty read at end");

    let oor = |offset, len| StorageError::OutOfRange {
        offset,
        len,
        size: 64,
    };
    assert_eq!(storage.read_exact_at(64, &mut [0u8; 1]), Err(oor(64, 1)));
    assert_eq!(storage.read_exact_at(63, &mut [0u8; 2]), Err(oor(63, 2)));
    assert_eq!(storage.read_exact_at(0, &mut [0u8; 65]), Err(oor(0, 65)));
    assert_eq!(storage.read_exact_at(65, &mut []), Err(oor(65, 0)));
    assert_eq!(
        storage.read_exact_at(u64::MAX, &mut []),
        Err(oor(u64::MAX, 0))
    );
    assert_eq!(
        storage.read_exact_at(u64::MAX, &mut [0u8; 1]),
        Err(StorageError::OffsetOverflow {
            offset: u64::MAX,
            len: 1
        })
    );
    assert_eq!(
        storage.read_exact_at(u64::MAX - 1, &mut [0u8; 4]),
        Err(StorageError::OffsetOverflow {
            offset: u64::MAX - 1,
            len: 4
        })
    );

    assert_eq!(storage.write_all_at(64, &[9]), Err(oor(64, 1)));
    assert_eq!(storage.write_all_at(63, &[9, 9]), Err(oor(63, 2)));
    assert_eq!(
        storage.write_all_at(u64::MAX, &[9]),
        Err(StorageError::OffsetOverflow {
            offset: u64::MAX,
            len: 1
        })
    );
    assert_eq!(
        read_all(&storage),
        initial,
        "rejected writes must not modify the object"
    );
}

fn model_based_random_io<F: ContractFixture>(fixture: &mut F) {
    let mut rng = Gen(0xC0FFEE);
    let mut model = rng.bytes(4093);
    let mut storage = checked(fixture.create(&model));
    let can_resize = storage.capabilities().resize;

    for step in 0..600u32 {
        match rng.below(10) {
            0 if can_resize => {
                let new_len = rng.below(6000);
                storage.set_len(new_len).expect("resize within limit");
                model.resize(new_len as usize, 0);
            }
            1..=7 => {
                let offset = rng.below(model.len() as u64 + 8);
                let len = rng.below(300) as usize;
                let data = rng.bytes(len);
                let result = storage.write_all_at(offset, &data);
                if offset + len as u64 <= model.len() as u64 {
                    result.expect("in-range write");
                    let start = offset as usize;
                    model[start..start + len].copy_from_slice(&data);
                } else {
                    assert_eq!(
                        result,
                        Err(StorageError::OutOfRange {
                            offset,
                            len: len as u64,
                            size: model.len() as u64
                        })
                    );
                }
            }
            _ => {
                let offset = rng.below(model.len() as u64 + 1);
                let len = rng.below(300).min(model.len() as u64 - offset) as usize;
                let mut got = vec![0xA5u8; len];
                storage
                    .read_exact_at(offset, &mut got)
                    .expect("in-range read");
                let start = offset as usize;
                assert_eq!(got, &model[start..start + len], "step {step}");
            }
        }
        if step % 50 == 0 {
            assert_eq!(read_all(&storage), model, "full compare at step {step}");
        }
    }
    assert_eq!(read_all(&storage), model);
    if let Some(external) = fixture.external_bytes() {
        assert_eq!(external, model, "bytes observed outside the backend");
    }
}

fn large_transfer<F: ContractFixture>(fixture: &mut F) {
    let mut rng = Gen(77);
    let mut model = vec![0u8; 400_000];
    let mut storage = checked(fixture.create(&model));
    let data = rng.bytes(300_001);
    storage.write_all_at(7, &data).expect("large write");
    model[7..7 + data.len()].copy_from_slice(&data);
    assert_eq!(read_all(&storage), model);
    if let Some(external) = fixture.external_bytes() {
        assert!(external == model, "large transfer reached the object");
    }
}

fn resize_behavior<F: ContractFixture>(fixture: &mut F) {
    let initial = Gen(5).bytes(100);
    let mut storage = checked(fixture.create(&initial));
    let caps = storage.capabilities();
    if !caps.resize {
        assert_eq!(storage.set_len(10), Err(StorageError::UnsupportedResize));
        assert_eq!(storage.size(), Ok(100));
        return;
    }
    // Shrink keeps the prefix.
    storage.set_len(40).expect("shrink");
    assert_eq!(read_all(&storage), &initial[..40]);
    // Growth reads back as explicit zero bytes, never stale data.
    storage.set_len(120).expect("grow");
    let mut expected = initial[..40].to_vec();
    expected.resize(120, 0);
    assert_eq!(read_all(&storage), expected);
    storage.set_len(120).expect("same size");
    storage.set_len(0).expect("to empty");
    assert_eq!(storage.size(), Ok(0));
    if let Some(limit) = caps.max_len.checked_add(1) {
        assert_eq!(
            storage.set_len(limit),
            Err(StorageError::ExceedsBackendLimit {
                requested: limit,
                max: caps.max_len
            })
        );
        assert_eq!(
            storage.size(),
            Ok(0),
            "rejected resize must not change size"
        );
    }
    if let Some(external) = fixture.external_bytes() {
        assert!(external.is_empty());
    }
}

fn read_only_behavior<F: ContractFixture>(fixture: &mut F) {
    let initial = Gen(9).bytes(100);
    let mut storage = checked(fixture.create_read_only(&initial));
    assert_eq!(read_all(&storage), initial);
    assert_eq!(storage.write_all_at(0, &[1]), Err(StorageError::ReadOnly));
    assert_eq!(storage.write_all_at(0, &[]), Err(StorageError::ReadOnly));
    assert_eq!(storage.set_len(10), Err(StorageError::ReadOnly));
    assert_eq!(read_all(&storage), initial);
    if let Some(external) = fixture.external_bytes() {
        assert_eq!(external, initial);
    }
}

fn durability_matches_capabilities<F: ContractFixture>(fixture: &mut F) {
    let mut storage = checked(fixture.create(&[1, 2, 3]));
    let caps = storage.capabilities();
    let file = storage.flush_file_durable();
    if caps.file_durability {
        assert_eq!(file.map(|a| a.scope()), Ok(DurabilityScope::File));
    } else {
        assert_eq!(
            file,
            Err(StorageError::UnsupportedDurability {
                scope: DurabilityScope::File
            })
        );
    }
    let namespace = storage.flush_namespace_durable();
    if caps.namespace_durability {
        assert_eq!(namespace.map(|a| a.scope()), Ok(DurabilityScope::Namespace));
    } else {
        assert_eq!(
            namespace,
            Err(StorageError::UnsupportedDurability {
                scope: DurabilityScope::Namespace
            })
        );
    }
}
