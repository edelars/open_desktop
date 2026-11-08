//! Tests of the storage I/O contract against the in-memory backend.

use crate::io::{
    BackendError, CapabilityInconsistency, CheckedStorage, ContractViolation, DurabilityScope,
    IoOp, MAX_INTERRUPTED_RETRIES, StorageBackend, StorageCapabilities, StorageError,
};
use crate::testing::contract::{ContractFixture, run_contract};
use crate::testing::memory::{Fault, MEMORY_MAX_LEN, MemoryStorage};
use std::cell::Cell;
use std::io::ErrorKind;

#[derive(Default)]
struct MemoryFixture {
    max_chunk: Option<usize>,
    interrupt_every: Option<u32>,
}

impl MemoryFixture {
    fn tune(&self, mut backend: MemoryStorage) -> MemoryStorage {
        if let Some(chunk) = self.max_chunk {
            backend.set_max_chunk(chunk);
        }
        if let Some(n) = self.interrupt_every {
            backend.set_interrupt_every(n);
        }
        backend
    }
}

impl ContractFixture for MemoryFixture {
    type Backend = MemoryStorage;

    fn create(&mut self, initial: &[u8]) -> MemoryStorage {
        self.tune(MemoryStorage::new(initial))
    }

    fn create_read_only(&mut self, initial: &[u8]) -> MemoryStorage {
        self.tune(MemoryStorage::new_read_only(initial))
    }
}

#[test]
fn memory_backend_passes_contract() {
    run_contract(&mut MemoryFixture::default());
}

#[test]
fn memory_backend_passes_contract_with_short_transfers() {
    run_contract(&mut MemoryFixture {
        max_chunk: Some(3),
        interrupt_every: None,
    });
}

#[test]
fn memory_backend_passes_contract_with_interrupts_and_short_transfers() {
    run_contract(&mut MemoryFixture {
        max_chunk: Some(7),
        interrupt_every: Some(2),
    });
}

fn storage(initial: &[u8]) -> CheckedStorage<MemoryStorage> {
    CheckedStorage::new(MemoryStorage::new(initial)).unwrap()
}

fn faulty(initial: &[u8], faults: &[Fault]) -> CheckedStorage<MemoryStorage> {
    let mut backend = MemoryStorage::new(initial);
    for fault in faults {
        backend.push_fault(*fault);
    }
    CheckedStorage::new(backend).unwrap()
}

#[test]
fn bounded_interrupts_are_retried_then_reported() {
    let ok = vec![Fault::Interrupted; MAX_INTERRUPTED_RETRIES as usize];
    let s = faulty(&[1, 2, 3, 4], &ok);
    let mut buf = [0u8; 4];
    s.read_exact_at(0, &mut buf)
        .expect("retries within the bound");
    assert_eq!(buf, [1, 2, 3, 4]);

    let too_many = vec![Fault::Interrupted; MAX_INTERRUPTED_RETRIES as usize + 1];
    let s = faulty(&[1, 2, 3, 4], &too_many);
    assert_eq!(
        s.read_exact_at(0, &mut [0u8; 4]),
        Err(StorageError::InterruptedNoProgress {
            op: IoOp::Read,
            offset: 0,
            completed: 0,
            attempts: MAX_INTERRUPTED_RETRIES + 1
        })
    );

    let mut s = faulty(&[0; 4], &too_many);
    assert_eq!(
        s.write_all_at(0, &[1, 2]),
        Err(StorageError::InterruptedNoProgress {
            op: IoOp::Write,
            offset: 0,
            completed: 0,
            attempts: MAX_INTERRUPTED_RETRIES + 1
        })
    );
    assert_eq!(
        s.into_backend().ok().expect("not sticky").bytes(),
        [0; 4],
        "no byte was written"
    );
}

#[test]
fn interrupt_counter_resets_after_progress() {
    // 10 interrupts, one byte of progress, 10 more: never 17 in a row.
    let mut faults = vec![Fault::Interrupted; 10];
    faults.push(Fault::Limit(1));
    faults.extend(vec![Fault::Interrupted; 10]);
    let s = faulty(&[5, 6, 7], &faults);
    let mut buf = [0u8; 3];
    s.read_exact_at(0, &mut buf)
        .expect("progress resets the counter");
    assert_eq!(buf, [5, 6, 7]);
}

#[test]
fn zero_progress_is_a_short_outcome_not_zero_fill() {
    let s = faulty(&[9; 8], &[Fault::ZeroProgress]);
    let mut buf = [0xEE; 8];
    assert_eq!(
        s.read_exact_at(0, &mut buf),
        Err(StorageError::ShortRead {
            offset: 0,
            requested: 8,
            completed: 0
        })
    );

    let mut s = faulty(&[0; 8], &[Fault::ZeroProgress]);
    assert_eq!(
        s.write_all_at(2, &[1, 2, 3]),
        Err(StorageError::ShortWrite {
            offset: 2,
            requested: 3,
            completed: 0
        })
    );
}

#[test]
fn partial_progress_then_zero_progress_reports_completed_bytes() {
    let mut s = faulty(&[0; 8], &[Fault::Limit(2), Fault::ZeroProgress]);
    assert_eq!(
        s.write_all_at(1, &[1, 2, 3, 4]),
        Err(StorageError::ShortWrite {
            offset: 1,
            requested: 4,
            completed: 2
        })
    );
    assert_eq!(
        s.into_backend().ok().expect("not sticky").bytes(),
        [0, 1, 2, 0, 0, 0, 0, 0]
    );

    let s = faulty(&[1, 2, 3, 4], &[Fault::Limit(3), Fault::ZeroProgress]);
    assert_eq!(
        s.read_exact_at(0, &mut [0u8; 4]),
        Err(StorageError::ShortRead {
            offset: 0,
            requested: 4,
            completed: 3
        })
    );
}

#[test]
fn object_shorter_than_reported_size_is_short_read() {
    let mut backend = MemoryStorage::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    backend.set_reported_size(20);
    let s = CheckedStorage::new(backend).unwrap();
    let mut buf = [0u8; 20];
    assert_eq!(
        s.read_exact_at(0, &mut buf),
        Err(StorageError::ShortRead {
            offset: 0,
            requested: 20,
            completed: 10
        })
    );
}

#[test]
fn io_error_reports_progress_made_before_failure() {
    let mut s = faulty(
        &[0; 8],
        &[Fault::Limit(3), Fault::Error(ErrorKind::StorageFull)],
    );
    assert_eq!(
        s.write_all_at(0, &[1; 8]),
        Err(StorageError::Io {
            op: IoOp::Write,
            kind: ErrorKind::StorageFull,
            os_error: None,
            completed: 3
        })
    );
    let s = faulty(&[7; 8], &[Fault::Error(ErrorKind::PermissionDenied)]);
    assert_eq!(
        s.read_exact_at(0, &mut [0u8; 8]),
        Err(StorageError::Io {
            op: IoOp::Read,
            kind: ErrorKind::PermissionDenied,
            os_error: None,
            completed: 0
        })
    );
}

#[test]
fn backend_reporting_too_many_bytes_is_a_contract_violation() {
    let s = faulty(&[1; 4], &[Fault::ReportTooMany]);
    assert_eq!(
        s.read_exact_at(0, &mut [0u8; 4]),
        Err(StorageError::BackendContract(
            ContractViolation::TransferExceedsRequest {
                op: IoOp::Read,
                requested: 4,
                returned: 5
            }
        ))
    );
    let mut s = faulty(&[0; 4], &[Fault::ReportTooMany]);
    assert_eq!(
        s.write_all_at(0, &[1; 4]),
        Err(StorageError::BackendContract(
            ContractViolation::TransferExceedsRequest {
                op: IoOp::Write,
                requested: 4,
                returned: 5
            }
        ))
    );
}

#[test]
fn rejected_calls_do_not_reach_the_backend() {
    // A queued fault would be consumed by any backend call.
    let mut s = faulty(&[0; 4], &[Fault::Error(ErrorKind::Other)]);
    assert_eq!(
        s.write_all_at(3, &[1, 2]),
        Err(StorageError::OutOfRange {
            offset: 3,
            len: 2,
            size: 4
        })
    );
    assert_eq!(
        s.read_exact_at(u64::MAX, &mut [0u8; 2]),
        Err(StorageError::OffsetOverflow {
            offset: u64::MAX,
            len: 2
        })
    );
    // The fault is still pending: the next valid call consumes it.
    assert!(matches!(
        s.write_all_at(0, &[1]),
        Err(StorageError::Io {
            kind: ErrorKind::Other,
            ..
        })
    ));
}

#[test]
fn inconsistent_declared_capabilities_are_rejected() {
    let namespace_only = StorageCapabilities {
        namespace_durability: true,
        ..StorageCapabilities::read_write(MEMORY_MAX_LEN)
    };
    assert!(matches!(
        CheckedStorage::new(MemoryStorage::with_capabilities(&[], namespace_only)),
        Err(StorageError::InconsistentCapabilities(
            CapabilityInconsistency::NamespaceDurabilityWithoutFileDurability
        ))
    ));
    let resize_read_only = StorageCapabilities {
        resize: true,
        ..StorageCapabilities::read_only(MEMORY_MAX_LEN)
    };
    assert!(matches!(
        CheckedStorage::new(MemoryStorage::with_capabilities(&[], resize_read_only)),
        Err(StorageError::InconsistentCapabilities(
            CapabilityInconsistency::ResizeWithoutWritable
        ))
    ));
}

#[test]
fn durability_default_is_unsupported_and_backend_is_not_asked() {
    let mut s = storage(&[1, 2, 3]);
    assert!(!s.capabilities().file_durability);
    assert!(!s.capabilities().namespace_durability);
    assert!(!s.capabilities().exclusive_lock);
    s.write_all_at(0, &[9]).expect("write completion");
    // Write completion is not a durable acknowledgement.
    assert_eq!(
        s.flush_file_durable(),
        Err(StorageError::UnsupportedDurability {
            scope: DurabilityScope::File
        })
    );
    assert_eq!(
        s.flush_namespace_durable(),
        Err(StorageError::UnsupportedDurability {
            scope: DurabilityScope::Namespace
        })
    );
}

/// Backend whose durable flush is scripted; counts every backend call so the
/// tests can prove that refused operations never reach it.
struct FlushScript {
    inner: MemoryStorage,
    flush_result: Result<(), BackendError>,
    flushes: std::rc::Rc<Cell<u32>>,
    mutations: std::rc::Rc<Cell<u32>>,
}

impl StorageBackend for FlushScript {
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
        self.mutations.set(self.mutations.get() + 1);
        self.inner.write_at(offset, buf)
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.mutations.set(self.mutations.get() + 1);
        self.inner.set_len(len)
    }

    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.flushes.set(self.flushes.get() + 1);
        self.flush_result
    }

    fn flush_namespace_durable(&mut self) -> Result<(), BackendError> {
        self.flushes.set(self.flushes.get() + 1);
        self.flush_result
    }
}

fn flush_script(
    flush_result: Result<(), BackendError>,
) -> (
    CheckedStorage<FlushScript>,
    std::rc::Rc<Cell<u32>>,
    std::rc::Rc<Cell<u32>>,
) {
    let flushes = std::rc::Rc::new(Cell::new(0));
    let mutations = std::rc::Rc::new(Cell::new(0));
    let claims_file = StorageCapabilities {
        file_durability: true,
        namespace_durability: true,
        ..StorageCapabilities::read_write(MEMORY_MAX_LEN)
    };
    let backend = FlushScript {
        inner: MemoryStorage::with_capabilities(&[0; 8], claims_file),
        flush_result,
        flushes: flushes.clone(),
        mutations: mutations.clone(),
    };
    (CheckedStorage::new(backend).unwrap(), flushes, mutations)
}

#[test]
fn failed_flush_makes_storage_reconcile_required_and_sticky() {
    let io_error = BackendError::Io {
        kind: ErrorKind::Other,
        os_error: Some(5),
    };
    let (mut s, flushes, mutations) = flush_script(Err(io_error));
    s.write_all_at(0, &[1]).expect("write completion");
    assert_eq!(mutations.get(), 1);
    assert!(matches!(
        s.flush_file_durable(),
        Err(StorageError::Io {
            op: IoOp::FlushFile,
            os_error: Some(5),
            ..
        })
    ));
    assert_eq!(flushes.get(), 1);
    // Sticky: no second flush reaches the backend and no ack is produced,
    // even though the backend would now succeed.
    assert_eq!(s.flush_file_durable(), Err(StorageError::ReconcileRequired));
    assert_eq!(
        s.flush_namespace_durable(),
        Err(StorageError::ReconcileRequired)
    );
    assert_eq!(
        s.write_all_at(0, &[2]),
        Err(StorageError::ReconcileRequired)
    );
    assert_eq!(s.set_len(4), Err(StorageError::ReconcileRequired));
    assert_eq!(flushes.get(), 1, "refused flushes never reach the backend");
    assert_eq!(mutations.get(), 1, "refused writes never reach the backend");
    // Reads stay available for inspection.
    let mut buf = [0u8; 1];
    s.read_exact_at(0, &mut buf).unwrap();
    assert_eq!(buf, [1]);
}

#[test]
fn contradicting_flush_is_also_reconcile_required() {
    let (mut s, flushes, mutations) = flush_script(Err(BackendError::Unsupported));
    assert_eq!(
        s.flush_file_durable(),
        Err(StorageError::InconsistentCapabilities(
            CapabilityInconsistency::ClaimedDurabilityUnsupported(DurabilityScope::File)
        ))
    );
    assert_eq!(flushes.get(), 1);
    assert_eq!(s.flush_file_durable(), Err(StorageError::ReconcileRequired));
    assert_eq!(
        s.write_all_at(0, &[1]),
        Err(StorageError::ReconcileRequired)
    );
    assert_eq!(flushes.get(), 1, "refused flush never reaches the backend");
    assert_eq!(
        mutations.get(),
        0,
        "refused write never reaches the backend"
    );
}

#[test]
fn failed_namespace_flush_is_sticky() {
    let (mut s, flushes, mutations) = flush_script(Err(BackendError::Io {
        kind: ErrorKind::Other,
        os_error: Some(5),
    }));
    assert!(matches!(
        s.flush_namespace_durable(),
        Err(StorageError::Io {
            op: IoOp::FlushNamespace,
            ..
        })
    ));
    assert_eq!(
        s.write_all_at(0, &[1]),
        Err(StorageError::ReconcileRequired)
    );
    assert_eq!(s.flush_file_durable(), Err(StorageError::ReconcileRequired));
    assert_eq!(flushes.get(), 1);
    assert_eq!(mutations.get(), 0);
}

#[test]
fn sticky_storage_does_not_release_the_backend() {
    let (mut s, _, _) = flush_script(Err(BackendError::Io {
        kind: ErrorKind::Other,
        os_error: Some(5),
    }));
    assert!(s.flush_file_durable().is_err());
    // into_backend returns the checked object itself; it is still sticky, so
    // the backend cannot be re-wrapped by `CheckedStorage::new`.
    let Err(mut s) = s.into_backend() else {
        panic!("backend released from sticky storage");
    };
    assert_eq!(
        s.write_all_at(0, &[1]),
        Err(StorageError::ReconcileRequired)
    );
}

#[test]
fn refusal_naming_another_scope_is_a_contract_violation_and_sticky() {
    let (mut s, _, mutations) = flush_script(Err(BackendError::DurabilityUnsupported {
        scope: DurabilityScope::Namespace,
    }));
    assert_eq!(
        s.flush_file_durable(),
        Err(StorageError::BackendContract(
            ContractViolation::DurabilityScopeMismatch {
                requested: DurabilityScope::File,
                reported: DurabilityScope::Namespace,
            }
        ))
    );
    assert_eq!(
        s.write_all_at(0, &[1]),
        Err(StorageError::ReconcileRequired)
    );
    assert_eq!(mutations.get(), 0);
}

#[test]
fn explicit_durability_refusal_is_not_an_ack_and_not_sticky() {
    let (mut s, flushes, _) = flush_script(Err(BackendError::DurabilityUnsupported {
        scope: DurabilityScope::File,
    }));
    for _ in 0..2 {
        assert_eq!(
            s.flush_file_durable(),
            Err(StorageError::UnsupportedDurability {
                scope: DurabilityScope::File
            })
        );
    }
    assert_eq!(flushes.get(), 2, "a refusal does not block later attempts");
    s.write_all_at(0, &[3]).expect("writes stay allowed");
}

#[test]
fn successful_flush_acks_and_does_not_set_reconcile() {
    let (mut s, _, _) = flush_script(Ok(()));
    assert_eq!(
        s.flush_file_durable().map(|a| a.scope()),
        Ok(DurabilityScope::File)
    );
    s.write_all_at(0, &[3]).unwrap();
}

#[test]
fn claimed_durability_must_be_backed_by_the_backend() {
    let claims_file = StorageCapabilities {
        file_durability: true,
        ..StorageCapabilities::read_write(MEMORY_MAX_LEN)
    };
    let mut lying =
        CheckedStorage::new(MemoryStorage::with_capabilities(&[0], claims_file)).unwrap();
    assert_eq!(
        lying.flush_file_durable(),
        Err(StorageError::InconsistentCapabilities(
            CapabilityInconsistency::ClaimedDurabilityUnsupported(DurabilityScope::File)
        ))
    );

    let mut backend = MemoryStorage::with_capabilities(&[0], claims_file);
    backend.set_flush_file_ok();
    let mut honest = CheckedStorage::new(backend).unwrap();
    assert_eq!(
        honest.flush_file_durable().map(|a| a.scope()),
        Ok(DurabilityScope::File)
    );
    // Namespace scope is still undeclared.
    assert_eq!(
        honest.flush_namespace_durable(),
        Err(StorageError::UnsupportedDurability {
            scope: DurabilityScope::Namespace
        })
    );

    let claims_both = StorageCapabilities {
        namespace_durability: true,
        ..claims_file
    };
    let mut lying_namespace =
        CheckedStorage::new(MemoryStorage::with_capabilities(&[0], claims_both)).unwrap();
    assert_eq!(
        lying_namespace.flush_namespace_durable(),
        Err(StorageError::InconsistentCapabilities(
            CapabilityInconsistency::ClaimedDurabilityUnsupported(DurabilityScope::Namespace)
        ))
    );
}

/// Backend with a scriptable misbehavior used for resize and size failures.
struct Odd {
    caps: StorageCapabilities,
    size: Cell<u64>,
    size_fails: bool,
    set_len_updates_size: bool,
    set_len_supported: bool,
}

impl StorageBackend for Odd {
    fn capabilities(&self) -> StorageCapabilities {
        self.caps
    }

    fn size(&self) -> Result<u64, BackendError> {
        if self.size_fails {
            Err(BackendError::Io {
                kind: ErrorKind::Other,
                os_error: Some(5),
            })
        } else {
            Ok(self.size.get())
        }
    }

    fn read_at(&self, _: u64, _: &mut [u8]) -> Result<usize, BackendError> {
        Ok(0)
    }

    fn write_at(&mut self, _: u64, buf: &[u8]) -> Result<usize, BackendError> {
        Ok(buf.len())
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        if !self.set_len_supported {
            return Err(BackendError::Unsupported);
        }
        if self.set_len_updates_size {
            self.size.set(len);
        }
        Ok(())
    }
}

fn odd() -> Odd {
    Odd {
        caps: StorageCapabilities::read_write(1000),
        size: Cell::new(10),
        size_fails: false,
        set_len_updates_size: true,
        set_len_supported: true,
    }
}

#[test]
fn resize_misbehavior_is_typed() {
    let mut s = CheckedStorage::new(Odd {
        set_len_updates_size: false,
        ..odd()
    })
    .unwrap();
    assert_eq!(
        s.set_len(20),
        Err(StorageError::BackendContract(
            ContractViolation::SizeAfterResize {
                expected: 20,
                actual: 10
            }
        ))
    );

    let mut s = CheckedStorage::new(Odd {
        set_len_supported: false,
        ..odd()
    })
    .unwrap();
    assert_eq!(
        s.set_len(20),
        Err(StorageError::InconsistentCapabilities(
            CapabilityInconsistency::ClaimedResizeUnsupported
        ))
    );

    let mut no_resize = CheckedStorage::new(Odd {
        caps: StorageCapabilities {
            resize: false,
            ..StorageCapabilities::read_write(1000)
        },
        ..odd()
    })
    .unwrap();
    assert_eq!(no_resize.set_len(20), Err(StorageError::UnsupportedResize));
}

#[test]
fn size_error_is_reported_before_any_transfer() {
    let mut s = CheckedStorage::new(Odd {
        size_fails: true,
        ..odd()
    })
    .unwrap();
    let expected = StorageError::Io {
        op: IoOp::Size,
        kind: ErrorKind::Other,
        os_error: Some(5),
        completed: 0,
    };
    assert_eq!(s.size(), Err(expected));
    assert_eq!(s.read_exact_at(0, &mut [0u8; 1]), Err(expected));
    assert_eq!(s.write_all_at(0, &[1]), Err(expected));
}
