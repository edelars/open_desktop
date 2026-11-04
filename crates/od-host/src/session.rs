//! Opt-in, thread-affine owner for a bounded S0-09 compiled-code session.
//!
//! This is a local Rust lifecycle slice. It does not connect VMHost IPC or the
//! V0 C ABI to a persistent session.

use crate::{
    MAX_PUBLISHED_ARENA_BLOCKS, PublishedArenaBackend, PublishedArenaBlock, PublishedArenaError,
};
use od_isa::{LongModeState, decode};
use od_jit::block::{AdmittedInstructionBoundaries, CodeView};
use od_jit::dispatch::{
    DispatchControl, DispatchError, DispatchResult, MAX_MULTI_ENTRY_CACHE_BLOCKS,
    MAX_MULTI_ENTRY_CACHE_CODE_BYTES, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES, MultiEntryBlockCache,
    MultiEntryCacheConfigError, MultiEntryCacheCounters, run_multi_cached_controlled,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use std::num::NonZeroU64;
use std::sync::atomic::AtomicU8;

/// Maximum immutable source fragment retained by one session, matching V0.
pub const MAX_SESSION_CODE_BYTES: usize = 4096;

const SESSION_CODE_BASE: u64 = 0;
const SESSION_CONTEXT_ID: u64 = 1;
const SESSION_IDENTITY: CodeIdentity = CodeIdentity {
    address_space: AddressSpaceId::new(NonZeroU64::MIN),
    mapping: MappingId::new(NonZeroU64::MIN),
    mapping_version: MappingVersion::new(NonZeroU64::MIN),
    backing: BackingId::new(NonZeroU64::MIN),
    backing_version: BackingVersion::new(NonZeroU64::MIN),
    executable: true,
};

/// Errors while copying and admitting an immutable S0-09 session fragment.
#[derive(Debug, Eq, PartialEq)]
pub enum SessionCreateError {
    CodeTooLarge { actual: usize, max: usize },
    CodeBounds(od_jit::block::BuildError),
    Decode(od_isa::DecodeError),
    Admission(od_jit::block::AdmissionError),
    Cache(MultiEntryCacheConfigError),
    Arena(PublishedArenaError),
}

/// An error returned by a single dispatch attempt. The caller decides whether
/// and when to make another call; this owner never retries execution.
#[derive(Debug, Eq, PartialEq)]
pub struct SessionRunError(pub DispatchError<PublishedArenaError>);

/// One synchronous dispatch result plus the cache accounting snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionRunResult {
    pub dispatch: DispatchResult,
    pub cache: MultiEntryCacheCounters,
}

/// Owns copied S0-09 code/admission, frozen fixture identity, a bounded cache,
/// and at most one arena backend. The `Rc`-based backend and published handles
/// keep this type `!Send` and `!Sync`; create, run, clear and drop stay local to
/// its owning thread.
pub struct Session {
    code: Vec<u8>,
    admission: AdmittedInstructionBoundaries,
    execution_context: ExecutionContextSnapshot,
    dependencies: Vec<CodeDependencySpan>,
    cache: MultiEntryBlockCache<PublishedArenaBlock>,
    backend: Option<PublishedArenaBackend>,
}

impl Session {
    /// Copy and admit one immutable S0-09 fragment, then acquire the process's
    /// single executable-memory lease for this retained session.
    pub fn new(code: &[u8]) -> Result<Self, SessionCreateError> {
        if code.len() > MAX_SESSION_CODE_BYTES {
            return Err(SessionCreateError::CodeTooLarge {
                actual: code.len(),
                max: MAX_SESSION_CODE_BYTES,
            });
        }
        let owned_code = code.to_vec();
        let code_view = CodeView::new(SESSION_CODE_BASE, &owned_code)
            .map_err(SessionCreateError::CodeBounds)?;
        let decoded = decode(&owned_code).map_err(SessionCreateError::Decode)?;
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(code_view, &decoded)
            .map_err(SessionCreateError::Admission)?;
        let execution_context = ExecutionContextSnapshot::s0_09_fixture(
            ExecutionContextId::from_u64(SESSION_CONTEXT_ID)
                .expect("the fixed session execution-context id is nonzero"),
        );
        let dependencies = if owned_code.is_empty() {
            Vec::new()
        } else {
            let code_end = code_view.code_end();
            vec![CodeDependencySpan {
                start_rip: SESSION_CODE_BASE,
                end_rip: code_end,
                identity: SESSION_IDENTITY,
            }]
        };
        let cache = MultiEntryBlockCache::new(
            MAX_MULTI_ENTRY_CACHE_BLOCKS,
            MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
            MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
        )
        .map_err(SessionCreateError::Cache)?;
        let backend = PublishedArenaBackend::new(MAX_PUBLISHED_ARENA_BLOCKS)
            .map_err(SessionCreateError::Arena)?;

        Ok(Self {
            code: owned_code,
            admission,
            execution_context,
            dependencies,
            cache,
            backend: Some(backend),
        })
    }

    /// Run once with borrowed invocation state, budget, and STOP atomic. None
    /// of those request values are retained in the session or cached artifact.
    pub fn run(
        &mut self,
        state: &mut LongModeState,
        budget: u64,
        stop_requested: &AtomicU8,
    ) -> Result<SessionRunResult, SessionRunError> {
        if self.backend.is_none() {
            self.backend = Some(
                PublishedArenaBackend::new(MAX_PUBLISHED_ARENA_BLOCKS)
                    .map_err(|error| SessionRunError(DispatchError::Executor(error)))?,
            );
        }
        let backend = self
            .backend
            .as_mut()
            .expect("the backend was created immediately above");
        let code = CodeView::new(SESSION_CODE_BASE, &self.code)
            .expect("the owned code bounds were validated during session creation");
        let dispatch = run_multi_cached_controlled(
            code,
            &self.admission,
            self.execution_context,
            &self.dependencies,
            state,
            budget,
            DispatchControl {
                stop_rip: None,
                stop_requested,
                event_requested: None,
            },
            &mut self.cache,
            backend,
        )
        .map_err(SessionRunError)?;
        Ok(SessionRunResult {
            dispatch,
            cache: self.cache.counters(),
        })
    }

    /// Drop cached code and release the MAP_JIT lease. The next `run` lazily
    /// reacquires a fresh backend and can report `MappingAlreadyLive`.
    pub fn clear(&mut self) {
        self.cache.clear();
        drop(self.backend.take());
    }

    pub fn resident_entries(&self) -> usize {
        self.cache.resident_entries()
    }

    pub fn cache_counters(&self) -> MultiEntryCacheCounters {
        self.cache.counters()
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    use od_jit::dispatch::DispatchExit;
    use od_jit::runtime::RuntimeExitTag;
    use std::sync::atomic::Ordering;

    const MOV_RAX_7: [u8; 10] = [0x48, 0xb8, 7, 0, 0, 0, 0, 0, 0, 0];

    #[test]
    fn same_code_reuses_publication_with_fresh_state_budget_and_stop() {
        let _guard = crate::tests::jit_test_lock();
        let mut session = Session::new(&MOV_RAX_7).unwrap();
        let stop = AtomicU8::new(0);
        let mut first = LongModeState {
            gpr: [0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            rip: 0,
            rflags: 2,
            ..LongModeState::default()
        };
        let cold = session.run(&mut first, 1, &stop).unwrap();
        assert_eq!(cold.dispatch.retired_instructions, 1);
        assert_eq!(cold.cache.misses, 1);
        assert_eq!(cold.cache.successful_publications, 1);

        let mut stopped = LongModeState {
            gpr: [99; 16],
            rip: 0,
            rflags: 2,
            ..LongModeState::default()
        };
        stop.store(1, Ordering::Release);
        let stopped_result = session.run(&mut stopped, 77, &stop).unwrap();
        assert_eq!(
            stopped_result.dispatch.exit,
            DispatchExit::Runtime(RuntimeExitTag::StopRequested)
        );
        assert_eq!(stopped.gpr[0], 99);
        assert_eq!(stopped.rip, 0);

        stop.store(0, Ordering::Release);
        let mut second = LongModeState {
            gpr: [42; 16],
            rip: 0,
            rflags: 2,
            ..LongModeState::default()
        };
        let hot = session.run(&mut second, 1, &stop).unwrap();
        assert_eq!(second.gpr[0], 7);
        assert_eq!(second.rip, 10);
        assert_eq!(hot.cache.hits, 1);
        assert_eq!(hot.cache.successful_publications, 1);
    }

    #[test]
    fn clear_releases_publication_and_mapping_lease() {
        let _guard = crate::tests::jit_test_lock();
        let mut session = Session::new(&MOV_RAX_7).unwrap();
        let stop = AtomicU8::new(0);
        let mut state = LongModeState {
            rip: 0,
            rflags: 2,
            ..LongModeState::default()
        };
        session.run(&mut state, 1, &stop).unwrap();
        assert_eq!(session.resident_entries(), 1);
        session.clear();
        assert_eq!(session.resident_entries(), 0);

        let replacement = Session::new(&MOV_RAX_7).unwrap();
        drop(replacement);
    }

    #[test]
    fn retained_session_rejects_a_second_executable_memory_owner() {
        let _guard = crate::tests::jit_test_lock();
        let session = Session::new(&MOV_RAX_7).unwrap();
        assert!(matches!(
            Session::new(&MOV_RAX_7),
            Err(SessionCreateError::Arena(
                PublishedArenaError::MappingAlreadyLive
            ))
        ));
        drop(session);
        let replacement = Session::new(&MOV_RAX_7).unwrap();
        drop(replacement);
    }

    #[test]
    fn invalid_entry_is_reported_without_an_automatic_retry() {
        let _guard = crate::tests::jit_test_lock();
        let mut session = Session::new(&MOV_RAX_7).unwrap();
        let stop = AtomicU8::new(0);
        let mut invalid = LongModeState {
            rip: 1,
            rflags: 2,
            ..LongModeState::default()
        };
        let error = session.run(&mut invalid, 1, &stop).unwrap_err();
        assert!(matches!(
            error.0,
            DispatchError::EntryNotAdmitted { rip: 1 }
        ));
        assert_eq!(session.cache_counters().successful_publications, 0);
    }
}
