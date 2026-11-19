#[cfg(not(feature = "lock-wait-metrics"))]
pub(crate) use std::sync::Mutex as MeasuredMutex;

#[cfg(feature = "lock-wait-metrics")]
pub(crate) use enabled::MeasuredMutex;
#[cfg(feature = "lock-wait-metrics")]
pub use enabled::{BoardLockWaitMetrics, MutexAcquisitionMetrics};

#[cfg(feature = "lock-wait-metrics")]
mod enabled {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::{LockResult, Mutex, MutexGuard};
    use std::time::Instant;

    /// Host wall time across all acquisitions of one mutex, including uncontended ones.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct MutexAcquisitionMetrics {
        pub acquisition_count: u64,
        pub acquisition_wall_ns: u64,
        pub max_acquisition_wall_ns: u64,
    }

    /// Host-side device locks of a single board.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct BoardLockWaitMetrics {
        pub device_table: MutexAcquisitionMetrics,
        pub apic_pair: MutexAcquisitionMetrics,
    }

    #[derive(Debug)]
    pub(crate) struct MeasuredMutex<T> {
        inner: Mutex<T>,
        acquisitions: AtomicU64,
        wall_ns: AtomicU64,
        max_wall_ns: AtomicU64,
    }

    impl<T> MeasuredMutex<T> {
        pub(crate) fn new(value: T) -> Self {
            Self {
                inner: Mutex::new(value),
                acquisitions: AtomicU64::new(0),
                wall_ns: AtomicU64::new(0),
                max_wall_ns: AtomicU64::new(0),
            }
        }

        pub(crate) fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
            let start = Instant::now();
            let guard = self.inner.lock();
            let elapsed_ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            // The guard (also inside a PoisonError) serializes writers. Only sample or
            // reset these counters when the board is quiescent.
            let count = self.acquisitions.load(Relaxed).saturating_add(1);
            let total = self.wall_ns.load(Relaxed).saturating_add(elapsed_ns);
            let max = self.max_wall_ns.load(Relaxed).max(elapsed_ns);
            self.acquisitions.store(count, Relaxed);
            self.wall_ns.store(total, Relaxed);
            self.max_wall_ns.store(max, Relaxed);
            guard
        }

        pub(crate) fn get_mut(&mut self) -> LockResult<&mut T> {
            self.inner.get_mut()
        }

        #[cfg(test)]
        pub(crate) fn try_lock(&self) -> std::sync::TryLockResult<MutexGuard<'_, T>> {
            self.inner.try_lock()
        }

        pub(crate) fn metrics(&self) -> MutexAcquisitionMetrics {
            MutexAcquisitionMetrics {
                acquisition_count: self.acquisitions.load(Relaxed),
                acquisition_wall_ns: self.wall_ns.load(Relaxed),
                max_acquisition_wall_ns: self.max_wall_ns.load(Relaxed),
            }
        }

        pub(crate) fn reset_metrics(&self) {
            self.acquisitions.store(0, Relaxed);
            self.wall_ns.store(0, Relaxed);
            self.max_wall_ns.store(0, Relaxed);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::PoisonError;

        #[test]
        fn counts_recovered_acquisitions_without_counting_exclusive_access() {
            let mut lock = MeasuredMutex::new(0);
            let other = MeasuredMutex::new(());
            *lock.get_mut().unwrap() = 1;
            assert_eq!(lock.metrics(), MutexAcquisitionMetrics::default());
            let panic = std::panic::catch_unwind(|| {
                let _guard = lock.lock().unwrap();
                panic!("poison the lock");
            });
            assert!(panic.is_err());
            assert_eq!(*lock.lock().unwrap_or_else(PoisonError::into_inner), 1);
            let metrics = lock.metrics();
            assert_eq!(metrics.acquisition_count, 2);
            assert!(metrics.max_acquisition_wall_ns <= metrics.acquisition_wall_ns);
            assert_eq!(other.metrics(), MutexAcquisitionMetrics::default());
            lock.reset_metrics();
            assert_eq!(lock.metrics(), MutexAcquisitionMetrics::default());
            assert!(lock.lock().is_err());
            assert_eq!(lock.metrics().acquisition_count, 1);
        }
    }
}
