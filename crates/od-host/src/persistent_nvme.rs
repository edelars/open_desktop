//! Persistent host-side NVMe backend over a locked RAW file.
//!
//! Each successful guest write is followed by a file-data durable flush before
//! this backend returns success. A runtime flush/write failure makes this
//! adapter terminal for its VM. Indeterminate durability failures retain the
//! checked storage and its writer lock until the adapter is dropped; no retry,
//! reopen, or recovery is attempted here.

use std::path::Path;

use od_machine::{BackendError, NvmeAttachError, NvmeBackend, NvmeHandle};
use od_storage::io::{CheckedStorage, DurabilityScope, StorageBackend, StorageError};
use od_storage::raw::{RawError, RawImage};

use crate::storage_io::{HostFileStorage, WriterOpenError};

const SECTOR_BYTES: usize = 512;
const SECTOR_BYTES_U64: u64 = SECTOR_BYTES as u64;

/// Failure while opening and validating a persistent RAW namespace.
#[derive(Debug)]
pub enum PersistentNvmeOpenError {
    WriterOpen(WriterOpenError),
    RawImage(RawError),
    FileDurabilityUnsupported,
}

impl std::fmt::Display for PersistentNvmeOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "persistent NVMe backend open failed: {self:?}")
    }
}

impl std::error::Error for PersistentNvmeOpenError {}

/// Failure while opening a persistent namespace and attaching it to a
/// disabled NVMe controller.
#[derive(Debug)]
pub enum AttachPersistentNvmeError {
    Open(PersistentNvmeOpenError),
    Attach(NvmeAttachError),
}

impl std::fmt::Display for AttachPersistentNvmeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "persistent NVMe attach failed: {self:?}")
    }
}

impl std::error::Error for AttachPersistentNvmeError {}

/// Locked file-backed NVMe namespace. Attach through
/// [`attach_persistent_nvme`] so the controller enforces `CC.EN = 0`.
pub struct PersistentNvmeBackend {
    inner: PersistentStorage<HostFileStorage>,
}

impl PersistentNvmeBackend {
    /// Opens an existing RAW image with an exclusive writer lock.
    ///
    /// The host must declare file-data durability before the VM can start.
    /// This checks the declared capability only; it does not claim that a
    /// particular filesystem or device honors the durability request.
    pub fn open(path: &Path) -> Result<Self, PersistentNvmeOpenError> {
        let file = HostFileStorage::open_read_write_exclusive(path)
            .map_err(PersistentNvmeOpenError::WriterOpen)?;
        let inner = PersistentStorage::open(file)?;
        Ok(Self { inner })
    }

    /// Whether a runtime error has permanently failed this adapter instance.
    pub fn is_terminal_failed(&self) -> bool {
        self.inner.is_terminal_failed()
    }

    /// Whether the active host lifecycle quarantined this backing after a
    /// possible mutation or indeterminate durability result. This is an
    /// in-memory state, not a persistent marker or recovery procedure.
    pub fn is_quarantined(&self) -> bool {
        self.inner.is_quarantined()
    }
}

/// Opens and attaches a persistent RAW backend using the controller's
/// `attach_backend` gate. The controller rejects attachment while `CC.EN=1`.
pub fn attach_persistent_nvme(
    handle: &NvmeHandle,
    path: &Path,
) -> Result<(), AttachPersistentNvmeError> {
    let backend = PersistentNvmeBackend::open(path).map_err(AttachPersistentNvmeError::Open)?;
    handle
        .attach_backend(Box::new(backend))
        .map_err(AttachPersistentNvmeError::Attach)
}

enum Held<B: StorageBackend> {
    Image { _image: RawImage<B> },
    Checked { _storage: CheckedStorage<B> },
}

enum State<B: StorageBackend> {
    Active(RawImage<B>),
    Failed {
        _held: Option<Held<B>>,
        quarantined: bool,
    },
}

/// Generic implementation kept private so only the locked host-file wrapper
/// can be attached in production. Unit tests use the existing FaultBackend.
struct PersistentStorage<B: StorageBackend> {
    state: State<B>,
    sectors: u64,
    guest_mutation_seen: bool,
}

impl<B: StorageBackend> PersistentStorage<B> {
    fn open(backend: B) -> Result<Self, PersistentNvmeOpenError> {
        let image = RawImage::open_read_write(backend, None, SECTOR_BYTES_U64)
            .map_err(PersistentNvmeOpenError::RawImage)?;
        if !image.capabilities().file_durability {
            return Err(PersistentNvmeOpenError::FileDurabilityUnsupported);
        }
        let sectors = image.block_count();
        Ok(Self {
            state: State::Active(image),
            sectors,
            guest_mutation_seen: false,
        })
    }

    fn is_terminal_failed(&self) -> bool {
        matches!(self.state, State::Failed { .. })
    }

    fn is_quarantined(&self) -> bool {
        matches!(
            self.state,
            State::Failed {
                quarantined: true,
                ..
            }
        )
    }

    fn take_active(&mut self) -> Result<RawImage<B>, BackendError> {
        match std::mem::replace(
            &mut self.state,
            State::Failed {
                _held: None,
                quarantined: true,
            },
        ) {
            State::Active(image) => Ok(image),
            failed @ State::Failed { .. } => {
                self.state = failed;
                Err(BackendError::Io)
            }
        }
    }

    fn fail_with_image(&mut self, image: RawImage<B>, quarantined: bool) {
        self.state = State::Failed {
            _held: Some(Held::Image { _image: image }),
            quarantined,
        };
    }

    fn fail_with_storage(&mut self, storage: CheckedStorage<B>, quarantined: bool) {
        self.state = State::Failed {
            _held: Some(Held::Checked { _storage: storage }),
            quarantined,
        };
    }

    fn reopen_after_ack(
        &mut self,
        storage: CheckedStorage<B>,
        logical_size: u64,
    ) -> Result<(), BackendError> {
        match RawImage::reopen_read_write_checked(storage, Some(logical_size), SECTOR_BYTES_U64) {
            Ok(image) => {
                self.state = State::Active(image);
                Ok(())
            }
            Err((storage, _error)) => {
                // Keep the original checked owner when metadata/geometry
                // validation fails, so dropping the failed RAW view cannot
                // release LOCK_EX while this adapter remains terminal.
                self.fail_with_storage(storage, true);
                Err(BackendError::Io)
            }
        }
    }

    fn durable_flush(&mut self) -> Result<(), BackendError> {
        let image = self.take_active()?;
        let logical_size = image.logical_size();
        let mut storage = image.into_storage();
        match storage.flush_file_durable() {
            Ok(ack) if ack.scope() == DurabilityScope::File => {
                drop(ack);
                self.reopen_after_ack(storage, logical_size)
            }
            Ok(_) => {
                self.fail_with_storage(storage, true);
                Err(BackendError::Io)
            }
            Err(error) => {
                let explicit_refusal = matches!(error, StorageError::UnsupportedDurability { .. });
                let quarantined = self.guest_mutation_seen || !explicit_refusal;
                self.fail_with_storage(storage, quarantined);
                Err(BackendError::Io)
            }
        }
    }

    fn range(&self, lba: u64, len: usize) -> Result<u64, BackendError> {
        if len == 0 || !len.is_multiple_of(SECTOR_BYTES) {
            return Err(BackendError::OutOfRange);
        }
        let count = u64::try_from(len / SECTOR_BYTES).map_err(|_| BackendError::OutOfRange)?;
        let end = lba.checked_add(count).ok_or(BackendError::OutOfRange)?;
        if end > self.sectors {
            return Err(BackendError::OutOfRange);
        }
        lba.checked_mul(SECTOR_BYTES_U64)
            .ok_or(BackendError::OutOfRange)
    }

    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError> {
        if self.is_terminal_failed() {
            return Err(BackendError::Io);
        }
        let offset = self.range(lba, buffer.len())?;
        match &self.state {
            State::Active(image) => image
                .read_aligned(offset, buffer)
                .map_err(|_| BackendError::Io),
            State::Failed { .. } => Err(BackendError::Io),
        }
    }

    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError> {
        if self.is_terminal_failed() {
            return Err(BackendError::Io);
        }
        let offset = self.range(lba, data.len())?;
        let mut image = self.take_active()?;
        if image.write_aligned(offset, data).is_err() {
            self.fail_with_image(image, true);
            return Err(BackendError::Io);
        }
        self.guest_mutation_seen = true;
        let logical_size = image.logical_size();
        let mut storage = image.into_storage();
        match storage.flush_file_durable() {
            Ok(ack) if ack.scope() == DurabilityScope::File => {
                drop(ack);
                self.reopen_after_ack(storage, logical_size)
            }
            Ok(_) => {
                self.fail_with_storage(storage, true);
                Err(BackendError::Io)
            }
            Err(error) => {
                // A write was already attempted. Even an explicit refusal
                // leaves the result unacknowledged and quarantines this RAW
                // lifecycle; an indeterminate flush remains sticky inside
                // CheckedStorage.
                let _explicit_refusal = matches!(error, StorageError::UnsupportedDurability { .. });
                self.fail_with_storage(storage, true);
                Err(BackendError::Io)
            }
        }
    }
}

impl<B: StorageBackend + Send> NvmeBackend for PersistentStorage<B> {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError> {
        PersistentStorage::read_sectors(self, lba, buffer)
    }

    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError> {
        PersistentStorage::write_sectors(self, lba, data)
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        self.durable_flush()
    }
}

impl NvmeBackend for PersistentNvmeBackend {
    fn sector_count(&self) -> u64 {
        self.inner.sector_count()
    }

    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError> {
        self.inner.read_sectors(lba, buffer)
    }

    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError> {
        self.inner.write_sectors(lba, data)
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use od_storage::testing::fault::{CrashPolicy, Fault, FaultHandle, FlushFault, WriteFault};

    #[cfg(target_os = "macos")]
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    #[cfg(target_os = "macos")]
    const CHILD_PATH_ENV: &str = "OD_S5_03_KILL_PATH";
    #[cfg(target_os = "macos")]
    const CHILD_MODE_ENV: &str = "OD_S5_03_KILL_MODE";
    #[cfg(target_os = "macos")]
    const CHILD_MARK: &str = "S5_03_EVENT=";

    #[cfg(target_os = "macos")]
    struct TempFile {
        path: PathBuf,
    }

    #[cfg(target_os = "macos")]
    impl TempFile {
        fn new(bytes: &[u8]) -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("od-s5-03-{}-{id}.raw", std::process::id()));
            std::fs::write(&path, bytes).expect("create test image");
            Self { path }
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn fault_adapter() -> (
        FaultHandle,
        PersistentStorage<od_storage::testing::fault::FaultBackend>,
    ) {
        let handle = FaultHandle::new(&vec![0x31; SECTOR_BYTES * 2], 0x503);
        let backend = PersistentStorage::open(handle.open()).expect("durable fault backend");
        (handle, backend)
    }

    #[test]
    fn missing_file_durability_refuses_backend_before_attachment() {
        let capabilities = od_storage::io::StorageCapabilities::read_write(
            od_storage::testing::memory::MEMORY_MAX_LEN,
        )
        .with_exclusive_lock();
        let backend = od_storage::testing::memory::MemoryStorage::with_capabilities(
            &[0x41; SECTOR_BYTES],
            capabilities,
        );
        assert!(matches!(
            PersistentStorage::open(backend),
            Err(PersistentNvmeOpenError::FileDurabilityUnsupported)
        ));
    }

    #[test]
    fn durable_write_is_visible_after_reopen_and_flush_always_runs() {
        let (handle, mut backend) = fault_adapter();
        let data = vec![0xa7; SECTOR_BYTES];
        backend.write_sectors(1, &data).expect("durable write");
        assert_eq!(handle.durable_bytes()[SECTOR_BYTES..], data);
        let flush_calls = || {
            handle
                .trace()
                .iter()
                .filter(|event| event.contains(" flush "))
                .count()
        };
        assert_eq!(flush_calls(), 1, "Write must perform one durable flush");

        drop(backend);
        let mut reopened = PersistentStorage::open(handle.open()).expect("reopen");
        let mut readback = vec![0; SECTOR_BYTES];
        reopened
            .read_sectors(1, &mut readback)
            .expect("read after reopen");
        assert_eq!(readback, data);

        reopened
            .durable_flush()
            .expect("flush even with no new write");
        assert_eq!(flush_calls(), 2, "runtime Flush must call backend flush");
    }

    #[test]
    fn invalid_range_is_rejected_before_mutation_and_does_not_fail_adapter() {
        let (handle, mut backend) = fault_adapter();
        assert_eq!(
            backend.write_sectors(2, &[0x44; SECTOR_BYTES]),
            Err(BackendError::OutOfRange)
        );
        assert!(!backend.is_terminal_failed());
        assert_eq!(handle.volatile_bytes(), handle.durable_bytes());
    }

    #[test]
    fn explicit_durability_refusal_before_guest_mutation_is_terminal_without_quarantine() {
        let (handle, mut backend) = fault_adapter();
        handle.script_next(Fault::Flush(FlushFault::Unsupported));
        assert_eq!(backend.durable_flush(), Err(BackendError::Io));
        assert!(backend.is_terminal_failed());
        assert!(!backend.is_quarantined());
        assert_eq!(handle.volatile_bytes(), handle.durable_bytes());
        assert_eq!(
            backend.write_sectors(0, &[0x55; SECTOR_BYTES]),
            Err(BackendError::Io),
            "terminal adapter must reject new writes even when RAW was unchanged"
        );
    }

    #[test]
    fn explicit_durability_refusal_after_write_quarantines_raw_lifecycle() {
        let (handle, mut backend) = fault_adapter();
        backend
            .write_sectors(0, &[0x61; SECTOR_BYTES])
            .expect("first write receives durable ack");
        handle.script_next(Fault::Flush(FlushFault::Unsupported));
        assert_eq!(backend.durable_flush(), Err(BackendError::Io));
        assert!(backend.is_terminal_failed());
        assert!(backend.is_quarantined());
        assert_eq!(handle.volatile_bytes(), handle.durable_bytes());
    }

    #[test]
    fn partial_write_failure_is_terminal_and_quarantines_raw_lifecycle() {
        let (handle, mut backend) = fault_adapter();
        handle.script_next(Fault::Write(WriteFault::TornIo { written: 128 }));
        assert_eq!(
            backend.write_sectors(0, &[0x55; SECTOR_BYTES]),
            Err(BackendError::Io)
        );
        assert!(backend.is_terminal_failed());
        assert!(backend.is_quarantined());
        assert_eq!(handle.volatile_bytes()[..128], [0x55; 128]);
        assert_eq!(handle.durable_bytes()[..128], [0x31; 128]);
        assert_eq!(
            backend.write_sectors(0, &[0x66; SECTOR_BYTES]),
            Err(BackendError::Io)
        );
        assert_eq!(
            backend.durable_flush(),
            Err(BackendError::Io),
            "terminal adapter must not retry flush"
        );
    }

    #[test]
    fn storage_full_write_error_is_terminal_and_quarantines_raw_lifecycle() {
        let (handle, mut backend) = fault_adapter();
        handle.script_next(Fault::Write(WriteFault::Fail(
            std::io::ErrorKind::StorageFull,
        )));
        assert_eq!(
            backend.write_sectors(0, &[0x53; SECTOR_BYTES]),
            Err(BackendError::Io)
        );
        assert!(backend.is_terminal_failed());
        assert!(backend.is_quarantined());
        assert_eq!(handle.volatile_bytes(), handle.durable_bytes());
    }

    #[test]
    fn indeterminate_flush_failure_is_terminal_and_never_returns_write_success() {
        let (handle, mut backend) = fault_adapter();
        handle.script(1, Fault::Flush(FlushFault::IoNotDurable));
        assert_eq!(
            backend.write_sectors(0, &[0x77; SECTOR_BYTES]),
            Err(BackendError::Io)
        );
        assert!(backend.is_terminal_failed());
        assert!(backend.is_quarantined());
        assert_eq!(
            backend.read_sectors(0, &mut [0; SECTOR_BYTES]),
            Err(BackendError::Io)
        );
        assert_eq!(
            handle.crash(CrashPolicy::DropPending),
            vec![0x31; SECTOR_BYTES * 2]
        );
    }

    #[test]
    fn lost_flush_ack_still_fails_write_even_if_model_made_bytes_durable() {
        let (handle, mut backend) = fault_adapter();
        handle.script(1, Fault::Flush(FlushFault::IoAfterDurable));
        assert_eq!(
            backend.write_sectors(0, &[0x88; SECTOR_BYTES]),
            Err(BackendError::Io)
        );
        assert!(backend.is_terminal_failed());
        assert!(backend.is_quarantined());
        assert_eq!(handle.durable_bytes()[..SECTOR_BYTES], [0x88; SECTOR_BYTES]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn host_file_write_reopens_with_data_and_holds_exclusive_writer_lock() {
        let file = TempFile::new(&[0x19; SECTOR_BYTES * 2]);
        let mut backend = PersistentNvmeBackend::open(&file.path).expect("persistent open");
        assert_eq!(backend.sector_count(), 2);
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&file.path),
            Err(WriterOpenError::WriterBusy)
        ));
        backend
            .write_sectors(1, &[0x92; SECTOR_BYTES])
            .expect("durable host write");
        drop(backend);

        let mut reopened = PersistentNvmeBackend::open(&file.path).expect("reopen after drop");
        let mut bytes = [0; SECTOR_BYTES];
        reopened.read_sectors(1, &mut bytes).expect("read back");
        assert_eq!(bytes, [0x92; SECTOR_BYTES]);
        drop(reopened);
        HostFileStorage::open_read_write_exclusive(&file.path).expect("lock released on drop");
    }

    #[cfg(target_os = "macos")]
    struct TruncateAfterFlushStorage {
        inner: HostFileStorage,
    }

    #[cfg(target_os = "macos")]
    impl StorageBackend for TruncateAfterFlushStorage {
        fn capabilities(&self) -> od_storage::io::StorageCapabilities {
            self.inner.capabilities()
        }
        fn size(&self) -> Result<u64, od_storage::io::BackendError> {
            self.inner.size()
        }
        fn read_at(
            &self,
            offset: u64,
            buffer: &mut [u8],
        ) -> Result<usize, od_storage::io::BackendError> {
            self.inner.read_at(offset, buffer)
        }
        fn write_at(
            &mut self,
            offset: u64,
            buffer: &[u8],
        ) -> Result<usize, od_storage::io::BackendError> {
            self.inner.write_at(offset, buffer)
        }
        fn set_len(&mut self, len: u64) -> Result<(), od_storage::io::BackendError> {
            self.inner.set_len(len)
        }
        fn flush_file_durable(&mut self) -> Result<(), od_storage::io::BackendError> {
            self.inner.flush_file_durable()?;
            // Simulate an external metadata change in the narrow interval
            // after durable acknowledgement and before RAW view reconstruction.
            self.inner.set_len(0)
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn failed_raw_reopen_keeps_writer_lock_in_terminal_adapter() {
        let file = TempFile::new(&[0x19; SECTOR_BYTES]);
        let writer = HostFileStorage::open_read_write_exclusive(&file.path).expect("writer");
        let mut backend = PersistentStorage::open(TruncateAfterFlushStorage { inner: writer })
            .expect("persistent open");

        assert_eq!(
            backend.write_sectors(0, &[0x92; SECTOR_BYTES]),
            Err(BackendError::Io),
            "size validation after flush must fail the command"
        );
        assert!(backend.is_terminal_failed());
        assert!(backend.is_quarantined());
        assert!(
            matches!(
                HostFileStorage::open_read_write_exclusive(&file.path),
                Err(WriterOpenError::WriterBusy)
            ),
            "the failed checked owner must retain LOCK_EX"
        );

        drop(backend);
        HostFileStorage::open_read_write_exclusive(&file.path)
            .expect("the OS lock is released only when the terminal owner drops");
    }

    #[cfg(target_os = "macos")]
    struct GatedHostFileStorage {
        inner: HostFileStorage,
        gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(target_os = "macos")]
    impl StorageBackend for GatedHostFileStorage {
        fn capabilities(&self) -> od_storage::io::StorageCapabilities {
            self.inner.capabilities()
        }
        fn size(&self) -> Result<u64, od_storage::io::BackendError> {
            self.inner.size()
        }
        fn read_at(
            &self,
            offset: u64,
            buffer: &mut [u8],
        ) -> Result<usize, od_storage::io::BackendError> {
            self.inner.read_at(offset, buffer)
        }
        fn write_at(
            &mut self,
            offset: u64,
            buffer: &[u8],
        ) -> Result<usize, od_storage::io::BackendError> {
            self.inner.write_at(offset, buffer)
        }
        fn set_len(&mut self, len: u64) -> Result<(), od_storage::io::BackendError> {
            self.inner.set_len(len)
        }
        fn flush_file_durable(&mut self) -> Result<(), od_storage::io::BackendError> {
            if self.gate.load(Ordering::Acquire) {
                use std::io::Write as _;
                let mut stdout = std::io::stdout().lock();
                writeln!(stdout, "{CHILD_MARK}DURABLE_FLUSH_PENDING").unwrap();
                stdout.flush().unwrap();
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
            }
            self.inner.flush_file_durable()
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn process_kill_child() {
        if std::env::var_os(CHILD_MODE_ENV).is_none() {
            return;
        }
        let path_value = std::env::var_os(CHILD_PATH_ENV).expect("child path");
        let path = Path::new(&path_value);
        let file = HostFileStorage::open_read_write_exclusive(path).expect("child writer lock");
        let gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut backend = PersistentStorage::open(GatedHostFileStorage {
            inner: file,
            gate: std::sync::Arc::clone(&gate),
        })
        .expect("child adapter open");
        gate.store(true, Ordering::Release);
        let result = backend.write_sectors(0, &[0xbb; SECTOR_BYTES]);
        println!("{CHILD_MARK}BACKEND_RETURNED_{result:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn killing_between_host_write_and_durable_ack_cannot_emit_write_success() {
        let file = TempFile::new(&[0x23; SECTOR_BYTES]);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "persistent_nvme::tests::process_kill_child",
                "--nocapture",
            ])
            .env(CHILD_PATH_ENV, &file.path)
            .env(CHILD_MODE_ENV, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn isolated writer process");
        let stdout = child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let marker = lines
            .by_ref()
            .map(Result::unwrap)
            .find(|line| line == &format!("{CHILD_MARK}DURABLE_FLUSH_PENDING"))
            .expect("child reached flush after the host write");
        assert_eq!(marker, format!("{CHILD_MARK}DURABLE_FLUSH_PENDING"));
        child.kill().expect("SIGKILL child before durable ack");
        let status = child.wait().expect("wait for killed child");
        assert!(!status.success(), "child must not exit normally");
        assert!(
            lines.next().is_none(),
            "child must not return backend success or publish a completion marker"
        );
        // The file is left as-is after process death; this test makes no claim
        // that unacknowledged bytes are durable or recoverable.
    }
}
