//! Optional timings for real cached-dispatch pipeline boundaries.
//!
//! The normal dispatch path uses [`NoopObserver`], whose generic methods
//! inline to no-ops. Wall-clock observation is only available with the
//! `phase-observation` feature and through an explicitly named entrypoint.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DispatchPhase {
    BlockBuildAndAdmission,
    KeySnapshotAndLookup,
    IrLowering,
    Allocation,
    EmissionAndFinalization,
    Publication,
    HostExecutionBoundary,
}

pub(crate) trait PhaseObserver {
    type Mark;

    fn begin(&mut self, phase: DispatchPhase) -> Self::Mark;

    fn end(&mut self, phase: DispatchPhase, mark: Self::Mark);
}

pub(crate) struct NoopObserver;

impl PhaseObserver for NoopObserver {
    type Mark = ();

    #[inline(always)]
    fn begin(&mut self, _phase: DispatchPhase) {}

    #[inline(always)]
    fn end(&mut self, _phase: DispatchPhase, _mark: Self::Mark) {}
}

#[inline(always)]
pub(crate) fn measure<T, O: PhaseObserver>(
    observer: &mut O,
    phase: DispatchPhase,
    operation: impl FnOnce() -> T,
) -> T {
    let mark = observer.begin(phase);
    let result = operation();
    observer.end(phase, mark);
    result
}

#[cfg(feature = "phase-observation")]
mod observed {
    use super::{DispatchPhase, PhaseObserver};
    use std::time::{Duration, Instant};

    const PHASE_COUNT: usize = 7;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ObservedDispatchPhase {
        BlockBuildAndAdmission,
        KeySnapshotAndLookup,
        IrLowering,
        Allocation,
        EmissionAndFinalization,
        Publication,
        HostExecutionBoundary,
    }

    impl ObservedDispatchPhase {
        const ALL: [Self; PHASE_COUNT] = [
            Self::BlockBuildAndAdmission,
            Self::KeySnapshotAndLookup,
            Self::IrLowering,
            Self::Allocation,
            Self::EmissionAndFinalization,
            Self::Publication,
            Self::HostExecutionBoundary,
        ];

        const fn index(self) -> usize {
            match self {
                Self::BlockBuildAndAdmission => 0,
                Self::KeySnapshotAndLookup => 1,
                Self::IrLowering => 2,
                Self::Allocation => 3,
                Self::EmissionAndFinalization => 4,
                Self::Publication => 5,
                Self::HostExecutionBoundary => 6,
            }
        }

        fn from_internal(phase: DispatchPhase) -> Self {
            match phase {
                DispatchPhase::BlockBuildAndAdmission => Self::BlockBuildAndAdmission,
                DispatchPhase::KeySnapshotAndLookup => Self::KeySnapshotAndLookup,
                DispatchPhase::IrLowering => Self::IrLowering,
                DispatchPhase::Allocation => Self::Allocation,
                DispatchPhase::EmissionAndFinalization => Self::EmissionAndFinalization,
                DispatchPhase::Publication => Self::Publication,
                DispatchPhase::HostExecutionBoundary => Self::HostExecutionBoundary,
            }
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct PhaseTotal {
        pub phase: ObservedDispatchPhase,
        pub calls: u64,
        pub elapsed_ns: u128,
    }

    #[derive(Clone, Debug)]
    pub struct PhaseRecorder {
        totals: [Duration; PHASE_COUNT],
        calls: [u64; PHASE_COUNT],
    }

    impl Default for PhaseRecorder {
        fn default() -> Self {
            Self {
                totals: [Duration::ZERO; PHASE_COUNT],
                calls: [0; PHASE_COUNT],
            }
        }
    }

    impl PhaseRecorder {
        pub fn totals(&self) -> Vec<PhaseTotal> {
            ObservedDispatchPhase::ALL
                .into_iter()
                .map(|phase| PhaseTotal {
                    phase,
                    calls: self.calls[phase.index()],
                    elapsed_ns: self.totals[phase.index()].as_nanos(),
                })
                .collect()
        }

        pub fn clear(&mut self) {
            *self = Self::default();
        }
    }

    impl PhaseObserver for PhaseRecorder {
        type Mark = Instant;

        #[inline(always)]
        fn begin(&mut self, _phase: DispatchPhase) -> Self::Mark {
            Instant::now()
        }

        #[inline(always)]
        fn end(&mut self, phase: DispatchPhase, mark: Self::Mark) {
            let phase = ObservedDispatchPhase::from_internal(phase);
            let index = phase.index();
            self.totals[index] = self.totals[index].saturating_add(mark.elapsed());
            self.calls[index] = self.calls[index].saturating_add(1);
        }
    }

    pub use ObservedDispatchPhase as PublicPhase;
    pub use PhaseRecorder as PublicPhaseRecorder;
    pub use PhaseTotal as PublicPhaseTotal;
}

#[cfg(feature = "phase-observation")]
pub use observed::{
    PublicPhase as ObservedDispatchPhase, PublicPhaseRecorder as PhaseRecorder,
    PublicPhaseTotal as PhaseTotal,
};
