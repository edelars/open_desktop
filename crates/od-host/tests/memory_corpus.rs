#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8};

use od_host::{
    CachedPublishedMemoryPairBackend, MemoryOperationExecutionResultV2, PublishedCode,
    PublishedMemoryBlockExecutor,
};
use od_isa::memory::{AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError};
use od_isa::{LongModeState, OperandSize};
use od_testkit::memory_corpus::{
    MEMORY_CORPUS_RAM_LEN, MEMORY_CORPUS_SEQUENCES, MemoryCorpusInput, MemoryCorpusSequence,
    S3_34A_CASES_PER_SEED, S3_34A_SEEDS, generate_memory_corpus_input,
};
use serde::{Deserialize, Serialize};

const ENTRY_RIP: u64 = 0x9e00;
const RAM_LEN: usize = MEMORY_CORPUS_RAM_LEN;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const CASES_PER_SEED: usize = S3_34A_CASES_PER_SEED;
const REPORT_VERSION: u32 = 1;
const ENVELOPE_VERSION: u32 = 1;
const CASE_PROFILE: &str = "s3-20j-two-site-qword-mov-v1";
const MAX_REDUCER_ATTEMPTS: usize = 512;
const SEEDS: [u64; 3] = S3_34A_SEEDS;
const SEQUENCES: [Sequence; 3] = MEMORY_CORPUS_SEQUENCES;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

type Sequence = MemoryCorpusSequence;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CpuSnapshot {
    gpr: [u64; 16],
    rflags: u64,
    rip: u64,
    auxiliary_state: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MemoryAccess {
    Read,
    Write,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum InjectedFailure {
    Architectural {
        vector: u8,
        error_code: Option<u32>,
        linear_address: Option<u64>,
    },
    Backend {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FaultInjection {
    site_index: u8,
    operation: MemoryAccess,
    failure: InjectedFailure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ContinuationProtocol {
    first_budget: u64,
    resume_instruction_index: u8,
    continuation_budget: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReplayProtocol {
    budget: u64,
    stop_requested: bool,
    stop_rip: Option<u64>,
    fault: Option<FaultInjection>,
    continuation: Option<ContinuationProtocol>,
}

impl ReplayProtocol {
    fn successful_pair() -> Self {
        Self {
            budget: 2,
            stop_requested: false,
            stop_rip: None,
            fault: None,
            continuation: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CaseEnvelope {
    version: u32,
    profile: String,
    seed: u64,
    case_id: usize,
    sequence: Sequence,
    source_bytes_hex: String,
    initial_cpu: CpuSnapshot,
    initial_ram_hex: String,
    first_address: u64,
    second_address: u64,
    protocol: ReplayProtocol,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct EnvironmentInfo {
    os: String,
    architecture: String,
    build_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BackendAvailability {
    native: bool,
    interpreter: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ObservationRecord {
    state_debug: String,
    gpr: [u64; 16],
    rflags: u64,
    rip: u64,
    ram_hex: String,
    callbacks: Vec<String>,
    #[serde(default)]
    memory_accesses: Vec<String>,
    status: String,
    remaining_budget: u64,
    retired: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReplayReport {
    report_version: u32,
    case: String,
    case_envelope: CaseEnvelope,
    environment: EnvironmentInfo,
    backend_availability: BackendAvailability,
    reason: String,
    native_results: Vec<ObservationRecord>,
    interpreter_results: Vec<ObservationRecord>,
    equivalent: bool,
    mismatch_classes: Vec<String>,
    #[serde(default)]
    reproduced_mismatch: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReducerRun {
    native_results: Vec<ObservationRecord>,
    interpreter_results: Vec<ObservationRecord>,
    mismatch_classes_by_phase: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReductionReport {
    reducer_version: u32,
    environment: EnvironmentInfo,
    backend_availability: BackendAvailability,
    original_envelope: CaseEnvelope,
    reduced_envelope: CaseEnvelope,
    original_mismatch_classes_by_phase: Vec<Vec<String>>,
    reduced_mismatch_classes_by_phase: Vec<Vec<String>>,
    initial_replays: Vec<ReducerRun>,
    reduced_run: ReducerRun,
    attempts: usize,
    attempt_limit: usize,
    stop_reason: String,
}

trait SequenceSiteOperation {
    fn site_operation(self, site_index: u8) -> MemoryAccess;
}

impl SequenceSiteOperation for Sequence {
    fn site_operation(self, site_index: u8) -> MemoryAccess {
        match (self, site_index) {
            (Self::StoreThenLoad, 0) | (Self::LoadThenStore, 1) => MemoryAccess::Write,
            (Self::StoreThenLoad, 1) | (Self::LoadThenStore, 0) | (Self::LoadThenLoad, 0 | 1) => {
                MemoryAccess::Read
            }
            _ => unreachable!("validated memory site index"),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CorpusInput {
    sequence: Sequence,
    gpr: [u64; 16],
    flags: u64,
    first_address: u64,
    second_address: u64,
    ram: [u8; RAM_LEN],
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation {
    state: LongModeState,
    ram: [u8; RAM_LEN],
    callbacks: Vec<&'static str>,
    memory_accesses: Vec<String>,
    memory_error: Option<MemoryError>,
    status: String,
    remaining_budget: u64,
    retired: u64,
}

#[derive(Clone)]
struct Ram {
    code: [u8; 6],
    bytes: [u8; RAM_LEN],
    callbacks: Vec<&'static str>,
    memory_accesses: Vec<String>,
    fail_read: Option<(usize, MemoryError)>,
    fail_write: Option<(usize, MemoryError)>,
    memory_error: Option<MemoryError>,
    event_on_access: Option<Arc<AtomicBool>>,
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address
            .checked_sub(ENTRY_RIP)
            .and_then(|n| usize::try_from(n).ok());
        offset
            .and_then(|n| self.code.get(n).copied())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        let read_site = self
            .callbacks
            .iter()
            .filter(|call| **call == "read")
            .count();
        self.callbacks.push("read");
        self.memory_accesses.push(format!("read@{address:#x}"));
        if let Some(event) = &self.event_on_access {
            event.store(true, std::sync::atomic::Ordering::Release);
        }
        if self
            .fail_read
            .as_ref()
            .is_some_and(|(site, _)| *site == read_site)
        {
            let error = self.fail_read.take().unwrap().1;
            self.memory_error = Some(error.clone());
            return Err(error);
        }
        let start = usize::try_from(address).map_err(|_| MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })?;
        let source = self
            .bytes
            .get(start..start.saturating_add(destination.len()))
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })?;
        destination.copy_from_slice(source);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        let write_site = self
            .callbacks
            .iter()
            .filter(|call| **call == "write")
            .count();
        self.callbacks.push("write");
        self.memory_accesses.push(format!("write@{address:#x}"));
        if let Some(event) = &self.event_on_access {
            event.store(true, std::sync::atomic::Ordering::Release);
        }
        if self
            .fail_write
            .as_ref()
            .is_some_and(|(site, _)| *site == write_site)
        {
            let error = self.fail_write.take().unwrap().1;
            self.memory_error = Some(error.clone());
            return Err(error);
        }
        let start = usize::try_from(address).map_err(|_| MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })?;
        let destination = self
            .bytes
            .get_mut(start..start.saturating_add(source.len()))
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })?;
        destination.copy_from_slice(source);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }
}

fn generated_input(seed: u64, index: usize, forced_sequence: Option<Sequence>) -> CorpusInput {
    generate_memory_corpus_input(seed, index, forced_sequence).into()
}

impl From<MemoryCorpusInput> for CorpusInput {
    fn from(input: MemoryCorpusInput) -> Self {
        Self {
            sequence: input.sequence,
            gpr: input.gpr,
            flags: input.flags,
            first_address: input.first_address,
            second_address: input.second_address,
            ram: input.ram,
        }
    }
}

impl CaseEnvelope {
    fn from_input(
        input: &CorpusInput,
        seed: u64,
        case_id: usize,
        protocol: ReplayProtocol,
    ) -> Self {
        Self {
            version: ENVELOPE_VERSION,
            profile: CASE_PROFILE.into(),
            seed,
            case_id,
            sequence: input.sequence,
            source_bytes_hex: hex(&input.sequence.bytes()),
            initial_cpu: CpuSnapshot {
                gpr: input.gpr,
                rflags: input.flags,
                rip: ENTRY_RIP,
                auxiliary_state: "default".into(),
            },
            initial_ram_hex: hex(&input.ram),
            first_address: input.first_address,
            second_address: input.second_address,
            protocol,
        }
    }

    fn validate(&self) -> Result<CorpusInput, String> {
        if self.version != ENVELOPE_VERSION {
            return Err(format!(
                "unsupported case envelope version {}",
                self.version
            ));
        }
        if self.profile != CASE_PROFILE {
            return Err(format!("unsupported replay profile {:?}", self.profile));
        }
        if self.source_bytes_hex != hex(&self.sequence.bytes()) {
            return Err("source bytes do not match the selected sequence profile".into());
        }
        if self.initial_cpu.rip != ENTRY_RIP {
            return Err(format!(
                "entry RIP must be {ENTRY_RIP:#x}, got {:#x}",
                self.initial_cpu.rip
            ));
        }
        if self.initial_cpu.auxiliary_state != "default" {
            return Err("unsupported non-default auxiliary CPU state".into());
        }
        let ram = decode_hex_array::<RAM_LEN>(&self.initial_ram_hex)
            .map_err(|e| format!("invalid RAM snapshot: {e}"))?;
        let first_address = self.initial_cpu.gpr[3];
        let second_address = match self.sequence {
            Sequence::StoreThenLoad | Sequence::LoadThenLoad => first_address,
            Sequence::LoadThenStore => self.initial_cpu.gpr[2],
        };
        if self.first_address != first_address || self.second_address != second_address {
            return Err("recorded access addresses do not match the initial GPR snapshot".into());
        }
        for (label, address) in [("first", first_address), ("second", second_address)] {
            let start = usize::try_from(address)
                .map_err(|_| format!("{label} qword address is outside fixture RAM"))?;
            if start.checked_add(8).is_none_or(|end| end > RAM_LEN) {
                return Err(format!("{label} qword access is outside fixture RAM"));
            }
        }
        validate_protocol(
            self.sequence,
            self.protocol.budget,
            &self.protocol,
            first_address,
            second_address,
        )?;
        Ok(CorpusInput {
            sequence: self.sequence,
            gpr: self.initial_cpu.gpr,
            flags: self.initial_cpu.rflags,
            first_address,
            second_address,
            ram,
        })
    }
}

fn validate_protocol(
    sequence: Sequence,
    budget: u64,
    protocol: &ReplayProtocol,
    first_address: u64,
    second_address: u64,
) -> Result<(), String> {
    if !(1..=2).contains(&budget) {
        return Err(format!("unsupported instruction budget {budget}"));
    }
    if let Some(stop_rip) = protocol.stop_rip
        && stop_rip != ENTRY_RIP
        && stop_rip != ENTRY_RIP + 3
    {
        return Err(format!("unsupported stop-RIP {stop_rip:#x}"));
    }
    if let Some(fault) = &protocol.fault {
        if fault.site_index > 1 {
            return Err("fault injection site index must be 0 or 1".into());
        }
        let expected_operation = sequence.site_operation(fault.site_index);
        if fault.operation != expected_operation {
            return Err("fault trigger operation does not match the selected memory site".into());
        }
        if protocol.stop_requested || protocol.stop_rip.is_some() || protocol.continuation.is_some()
        {
            return Err("fault injection cannot be combined with STOP or continuation".into());
        }
        let site_address = if fault.site_index == 0 {
            first_address
        } else {
            second_address
        };
        if let InjectedFailure::Architectural {
            linear_address: Some(address),
            ..
        } = fault.failure
            && address != site_address
        {
            return Err("architectural fault address does not match the selected site".into());
        }
    }
    if let Some(continuation) = &protocol.continuation {
        if protocol.budget != 1
            || continuation.first_budget != 1
            || continuation.resume_instruction_index != 1
            || continuation.continuation_budget != 1
            || protocol.stop_requested
            || protocol.stop_rip.is_some()
            || protocol.fault.is_some()
        {
            return Err("unsupported continuation protocol".into());
        }
    }
    Ok(())
}

fn decode_hex_array<const N: usize>(value: &str) -> Result<[u8; N], String> {
    if !value.is_ascii() || value.len() != N * 2 {
        return Err(format!("expected {} hex characters", N * 2));
    }
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| format!("invalid hex byte at offset {index}"))?;
    }
    Ok(bytes)
}

impl InjectedFailure {
    fn into_memory_error(&self) -> MemoryError {
        match self {
            Self::Architectural {
                vector,
                error_code,
                linear_address,
            } => MemoryError::Architectural(ArchitecturalFault {
                vector: *vector,
                error_code: *error_code,
                linear_address: *linear_address,
            }),
            Self::Backend { message } => MemoryError::Backend(message.clone()),
        }
    }
}

impl EnvironmentInfo {
    fn current() -> Self {
        let build_revision = std::env::var("OD_JIT_BUILD_REVISION")
            .ok()
            .or_else(|| {
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            })
            .unwrap_or_else(|| "unknown".into());
        Self {
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            build_revision,
        }
    }
}

impl BackendAvailability {
    fn current() -> Self {
        Self {
            native: native_backend_available(),
            interpreter: true,
        }
    }
}

fn native_backend_available() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

impl Observation {
    fn to_record(&self) -> ObservationRecord {
        ObservationRecord {
            state_debug: format!("{:?}", self.state),
            gpr: self.state.gpr,
            rflags: self.state.rflags,
            rip: self.state.rip,
            ram_hex: hex(&self.ram),
            callbacks: self
                .callbacks
                .iter()
                .map(|call| (*call).to_owned())
                .collect(),
            memory_accesses: self.memory_accesses.clone(),
            status: self.status.clone(),
            remaining_budget: self.remaining_budget,
            retired: self.retired,
        }
    }
}

fn initial_state(input: &CorpusInput, rip: u64) -> LongModeState {
    LongModeState {
        gpr: input.gpr,
        rip,
        rflags: input.flags,
        ..Default::default()
    }
}

fn initial_ram(input: &CorpusInput) -> Ram {
    Ram {
        code: input.sequence.bytes(),
        bytes: input.ram,
        callbacks: Vec::new(),
        memory_accesses: Vec::new(),
        fail_read: None,
        fail_write: None,
        memory_error: None,
        event_on_access: None,
    }
}

fn inject_fault(ram: &mut Ram, sequence: Sequence, site_index: u8, error: MemoryError) {
    let operation = sequence.site_operation(site_index);
    let ordinal = if site_index == 1 && sequence.site_operation(0) == operation {
        1
    } else {
        0
    };
    match operation {
        MemoryAccess::Read => ram.fail_read = Some((ordinal, error)),
        MemoryAccess::Write => ram.fail_write = Some((ordinal, error)),
    }
}

fn outcome(
    state: LongModeState,
    ram: Ram,
    status: String,
    remaining_budget: u64,
    retired: u64,
) -> Observation {
    Observation {
        state,
        ram: ram.bytes,
        callbacks: ram.callbacks,
        memory_accesses: ram.memory_accesses,
        memory_error: ram.memory_error,
        status,
        remaining_budget,
        retired,
    }
}

fn native_run(
    code: &PublishedCode,
    input: &CorpusInput,
    budget: u64,
    stop_rip: Option<u64>,
    stop: &AtomicU8,
    fault: Option<(u8, MemoryError)>,
) -> Observation {
    let mut state = initial_state(input, ENTRY_RIP);
    let mut ram = initial_ram(input);
    match fault {
        Some((site_index, error @ (MemoryError::Architectural(_) | MemoryError::Backend(_)))) => {
            inject_fault(&mut ram, input.sequence, site_index, error);
        }
        Some(_) => panic!("unexpected fault injection"),
        None => {}
    }
    // SAFETY: this fixture owns private synchronous RAM separate from its code
    // bytes; its qword writes are all-or-nothing and it has no concurrent users.
    match unsafe {
        code.execute_ir_memory_two_site_v2(
            &mut state,
            &mut ram,
            AccessContext::default(),
            budget,
            stop_rip,
            stop,
        )
    } {
        Ok(MemoryOperationExecutionResultV2 {
            exit,
            remaining_budget,
            retired_instructions,
        }) => outcome(
            state,
            ram,
            format!("{exit:?}"),
            remaining_budget,
            retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), budget, 0),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn normalize_dispatch_exit(exit: od_jit::memory_dispatch::BoundedMemoryExit) -> String {
    match exit {
        od_jit::memory_dispatch::BoundedMemoryExit::BoundaryReached { rip }
            if rip == ENTRY_RIP + 6 =>
        {
            "Complete".to_owned()
        }
        other => format!("{other:?}"),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn uncached_dispatch_run(
    input: &CorpusInput,
    budget: u64,
    stop_requested: bool,
    stop_rip: Option<u64>,
    event: Option<&Arc<AtomicBool>>,
    fault: Option<(u8, MemoryError)>,
) -> Observation {
    let code_bytes = input.sequence.bytes();
    let code = od_jit::block::CodeView::new(ENTRY_RIP, &code_bytes).unwrap();
    let boundary =
        od_jit::memory_dispatch::FixtureExecutionBoundary::new(code, ENTRY_RIP + 6).unwrap();
    let mut state = initial_state(input, ENTRY_RIP);
    let mut ram = initial_ram(input);
    ram.event_on_access = event.cloned();
    if let Some((site_index, error)) = fault {
        inject_fault(&mut ram, input.sequence, site_index, error);
    }
    let stop = AtomicU8::new(u8::from(stop_requested));
    let mut executor = PublishedMemoryBlockExecutor;
    // SAFETY: corpus RAM is private synchronous ordinary RAM, distinct from
    // immutable code bytes, with all-or-nothing writes and no concurrent users.
    match unsafe {
        od_jit::memory_dispatch::run_controlled(
            code,
            boundary,
            &mut state,
            &mut ram,
            AccessContext::default(),
            budget,
            stop_rip,
            &stop,
            event.map(Arc::as_ref),
            &mut executor,
        )
    } {
        Ok(result) => outcome(
            state,
            ram,
            normalize_dispatch_exit(result.exit),
            result.remaining_budget,
            result.retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), budget, 0),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn cached_dispatch_run(
    cache: &mut od_jit::memory_cache::MemoryPairCache<od_host::PublishedMemoryPairHandle>,
    backend: &mut CachedPublishedMemoryPairBackend,
    profile: od_jit::memory_cache::MemoryPairProfile,
    context_generation: u64,
    input: &CorpusInput,
    budget: u64,
    stop_requested: bool,
    stop_rip: Option<u64>,
    event: Option<&Arc<AtomicBool>>,
    fault: Option<(u8, MemoryError)>,
) -> Observation {
    cached_dispatch_run_with_dependency_version(
        cache,
        backend,
        profile,
        context_generation,
        1,
        input,
        budget,
        stop_requested,
        stop_rip,
        event,
        fault,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn cached_dispatch_run_with_dependency_version(
    cache: &mut od_jit::memory_cache::MemoryPairCache<od_host::PublishedMemoryPairHandle>,
    backend: &mut CachedPublishedMemoryPairBackend,
    profile: od_jit::memory_cache::MemoryPairProfile,
    context_generation: u64,
    dependency_version: u64,
    input: &CorpusInput,
    budget: u64,
    stop_requested: bool,
    stop_rip: Option<u64>,
    event: Option<&Arc<AtomicBool>>,
    fault: Option<(u8, MemoryError)>,
) -> Observation {
    use std::num::NonZeroU64;

    use od_jit::{
        memory_cache::MemoryExecutionContextGeneration,
        translation::{
            AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, MappingId,
            MappingVersion,
        },
    };

    let code_bytes = input.sequence.bytes();
    let code = od_jit::block::CodeView::new(ENTRY_RIP, &code_bytes).unwrap();
    let dependencies = [CodeDependencySpan {
        start_rip: ENTRY_RIP,
        end_rip: ENTRY_RIP + 6,
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
            mapping_version: MappingVersion::new(NonZeroU64::new(dependency_version).unwrap()),
            backing: BackingId::new(NonZeroU64::new(1).unwrap()),
            backing_version: BackingVersion::new(NonZeroU64::new(dependency_version).unwrap()),
            executable: true,
        },
    }];
    let mut state = initial_state(input, ENTRY_RIP);
    let mut ram = initial_ram(input);
    ram.event_on_access = event.cloned();
    if let Some((site_index, error)) = fault {
        inject_fault(&mut ram, input.sequence, site_index, error);
    }
    let stop = AtomicU8::new(u8::from(stop_requested));
    let generation = MemoryExecutionContextGeneration::from_u64(context_generation).unwrap();
    // SAFETY: corpus RAM is private synchronous ordinary RAM, distinct from
    // immutable code bytes, with all-or-nothing writes and no concurrent users.
    match unsafe {
        cache.run(
            profile,
            generation,
            code,
            &dependencies,
            &mut state,
            &mut ram,
            AccessContext::default(),
            budget,
            stop_rip,
            &stop,
            event.map(Arc::as_ref),
            backend,
        )
    } {
        Ok(result) => outcome(
            state,
            ram,
            normalize_dispatch_exit(result.exit),
            result.remaining_budget,
            result.retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), budget, 0),
    }
}

fn native_continue(code: &PublishedCode, input: &CorpusInput, prior: &Observation) -> Observation {
    let mut state = prior.state.clone();
    let mut ram = initial_ram(input);
    ram.bytes = prior.ram;
    ram.callbacks.clone_from(&prior.callbacks);
    ram.memory_accesses.clone_from(&prior.memory_accesses);
    let stop = AtomicU8::new(0);
    // SAFETY: the continuation uses the same private synchronous fixture RAM
    // contract, carrying forward the committed snapshot from the first entry.
    match unsafe {
        code.execute_ir_memory_two_site_v2(
            &mut state,
            &mut ram,
            AccessContext::default(),
            1,
            None,
            &stop,
        )
    } {
        Ok(result) => outcome(
            state,
            ram,
            format!("{:?}", result.exit),
            result.remaining_budget,
            result.retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), 1, 0),
    }
}

fn interpreter_run(
    input: &CorpusInput,
    budget: u64,
    stop_rip: Option<u64>,
    stop_requested: bool,
    fault: Option<(u8, MemoryError)>,
) -> Observation {
    let mut state = initial_state(input, ENTRY_RIP);
    let mut ram = initial_ram(input);
    match fault {
        Some((site_index, error @ (MemoryError::Architectural(_) | MemoryError::Backend(_)))) => {
            inject_fault(&mut ram, input.sequence, site_index, error);
        }
        Some(_) => panic!("unexpected fault injection"),
        None => {}
    }
    let result = od_isa::execute::run(
        &mut state,
        &mut ram,
        od_isa::execute::RunConfig {
            instruction_budget: budget,
            stop_rip,
            context: AccessContext::default(),
        },
        || stop_requested,
    );
    let status = match result.exit {
        od_isa::execute::RunExit::Fault(fault) => format!("GuestFault({fault:?})"),
        od_isa::execute::RunExit::InstructionBudgetExhausted
            if result.instructions_executed == 2 && state.rip == ENTRY_RIP + 6 =>
        {
            "Complete".into()
        }
        od_isa::execute::RunExit::InstructionBudgetExhausted => "BudgetExhausted".into(),
        other => format!("{other:?}"),
    };
    outcome(
        state,
        ram,
        status,
        budget.saturating_sub(result.instructions_executed),
        result.instructions_executed,
    )
}

fn interpreter_continue(input: &CorpusInput, prior: &Observation) -> Observation {
    let mut state = prior.state.clone();
    let mut ram = initial_ram(input);
    ram.bytes = prior.ram;
    ram.callbacks.clone_from(&prior.callbacks);
    ram.memory_accesses.clone_from(&prior.memory_accesses);
    let status = match od_isa::execute::step(&mut state, &mut ram, AccessContext::default()) {
        Ok(od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched) => {
            "Complete".to_owned()
        }
        Ok(od_isa::execute::StepOutcome::Fault(fault)) => format!("GuestFault({fault:?})"),
        Ok(other) => format!("{other:?}"),
        Err(error) => format!("error:{error:?}"),
    };
    outcome(state, ram, status, 0, 1)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn uncached_dispatch_continue(input: &CorpusInput, prior: &Observation) -> Observation {
    let code_bytes = input.sequence.bytes();
    let code = od_jit::block::CodeView::new(ENTRY_RIP, &code_bytes).unwrap();
    let boundary =
        od_jit::memory_dispatch::FixtureExecutionBoundary::new(code, ENTRY_RIP + 6).unwrap();
    let mut state = prior.state.clone();
    let mut ram = initial_ram(input);
    ram.bytes = prior.ram;
    ram.callbacks.clone_from(&prior.callbacks);
    ram.memory_accesses.clone_from(&prior.memory_accesses);
    let stop = AtomicU8::new(0);
    let mut executor = PublishedMemoryBlockExecutor;
    // SAFETY: continuation preserves the private synchronous corpus RAM
    // contract and resumes from actual RIP without replaying the prefix.
    match unsafe {
        od_jit::memory_dispatch::run_controlled(
            code,
            boundary,
            &mut state,
            &mut ram,
            AccessContext::default(),
            1,
            None,
            &stop,
            None,
            &mut executor,
        )
    } {
        Ok(result) => outcome(
            state,
            ram,
            normalize_dispatch_exit(result.exit),
            result.remaining_budget,
            result.retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), 1, 0),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn assert_matches(
    seed: u64,
    case_id: usize,
    input: &CorpusInput,
    native: &Observation,
    interp: &Observation,
) {
    let envelope =
        CaseEnvelope::from_input(input, seed, case_id, ReplayProtocol::successful_pair());
    assert_matches_named("generated", &envelope, native, interp);
}

fn mismatch_classes(native: &Observation, interp: &Observation) -> Vec<String> {
    let mut classes = Vec::new();
    for (index, (left, right)) in native.state.gpr.iter().zip(&interp.state.gpr).enumerate() {
        if left != right {
            classes.push(format!("gpr[{index}]"));
        }
    }
    if native.state.rflags != interp.state.rflags {
        classes.push("rflags".into());
    }
    if native.state.rip != interp.state.rip {
        classes.push("rip".into());
    }
    for (index, (left, right)) in native.ram.iter().zip(&interp.ram).enumerate() {
        if left != right {
            classes.push(format!("ram[{index}]"));
        }
    }
    if native.callbacks != interp.callbacks {
        classes.push("callbacks".into());
    }
    if native.memory_accesses != interp.memory_accesses {
        classes.push("memory_accesses".into());
    }
    if native.status != interp.status {
        classes.push("exit".into());
    }
    if native.remaining_budget != interp.remaining_budget {
        classes.push("remaining_budget".into());
    }
    if native.retired != interp.retired {
        classes.push("retirement".into());
    }
    if classes.is_empty() && native.state != interp.state {
        classes.push("auxiliary_cpu_state".into());
    }
    classes
}

fn compare_observations(native: &Observation, interp: &Observation) -> Result<(), Vec<String>> {
    let classes = mismatch_classes(native, interp);
    if classes.is_empty() {
        Ok(())
    } else {
        Err(classes)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn assert_matches_named(
    case: &str,
    envelope: &CaseEnvelope,
    native: &Observation,
    interp: &Observation,
) {
    if compare_observations(native, interp).is_ok() {
        return;
    }
    save_mismatch(case, envelope, native, interp, "backend results differ");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn assert_matches_named_phases(
    case: &str,
    envelope: &CaseEnvelope,
    native: &[Observation],
    interp: &[Observation],
) {
    assert_eq!(native.len(), interp.len(), "phase snapshots must be paired");
    if actual_mismatch_classes(native, interp).is_empty() {
        return;
    }
    save_mismatch_phases(case, envelope, native, interp, "backend results differ");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn assert_named_condition(
    case: &str,
    envelope: &CaseEnvelope,
    native: &Observation,
    interp: &Observation,
    condition: bool,
    reason: &str,
) {
    if !condition {
        save_mismatch(case, envelope, native, interp, reason);
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn assert_named_condition_phases(
    case: &str,
    envelope: &CaseEnvelope,
    native: &[Observation],
    interp: &[Observation],
    condition: bool,
    reason: &str,
) {
    if !condition {
        save_mismatch_phases(case, envelope, native, interp, reason);
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn save_mismatch(
    case: &str,
    envelope: &CaseEnvelope,
    native: &Observation,
    interp: &Observation,
    reason: &str,
) -> ! {
    save_mismatch_phases(
        case,
        envelope,
        std::slice::from_ref(native),
        std::slice::from_ref(interp),
        reason,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn save_mismatch_phases(
    case: &str,
    envelope: &CaseEnvelope,
    native: &[Observation],
    interp: &[Observation],
    reason: &str,
) -> ! {
    let report = mismatch_report(case, envelope, native, interp, reason);
    let path = std::env::temp_dir().join(format!(
        "od-host-memory-corpus-{:016x}-{}-{case}.json",
        envelope.seed, envelope.case_id
    ));
    let write_result = std::fs::write(&path, &report);
    panic!(
        "memory corpus mismatch case={case} seed={:#018x} case_id={}; saved={path:?}; write={write_result:?}\n{report}",
        envelope.seed, envelope.case_id
    );
}

fn mismatch_report(
    case: &str,
    envelope: &CaseEnvelope,
    native: &[Observation],
    interp: &[Observation],
    reason: &str,
) -> String {
    assert_eq!(native.len(), interp.len(), "phase snapshots must be paired");
    let expected_phases = if envelope.protocol.continuation.is_some() {
        2
    } else {
        1
    };
    assert_eq!(
        native.len(),
        expected_phases,
        "phase count must match protocol"
    );
    let mismatch_classes = actual_mismatch_classes(native, interp);
    let report = ReplayReport {
        report_version: REPORT_VERSION,
        case: case.into(),
        case_envelope: envelope.clone(),
        environment: EnvironmentInfo::current(),
        backend_availability: BackendAvailability::current(),
        reason: reason.into(),
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interp.iter().map(Observation::to_record).collect(),
        equivalent: mismatch_classes.is_empty(),
        mismatch_classes,
        reproduced_mismatch: None,
    };
    serde_json::to_string_pretty(&report).expect("replay report is serializable")
}

fn sample_report(
    envelope: CaseEnvelope,
    backend_availability: BackendAvailability,
) -> ReplayReport {
    let input = envelope
        .validate()
        .expect("test case envelope must validate");
    let observation = Observation {
        state: initial_state(&input, ENTRY_RIP),
        ram: input.ram,
        callbacks: Vec::new(),
        memory_accesses: Vec::new(),
        memory_error: None,
        status: "SavedInput".into(),
        remaining_budget: envelope.protocol.budget,
        retired: 0,
    };
    let record = observation.to_record();
    let phase_count = if envelope.protocol.continuation.is_some() {
        2
    } else {
        1
    };
    ReplayReport {
        report_version: REPORT_VERSION,
        case: "saved_input".into(),
        case_envelope: envelope,
        environment: EnvironmentInfo::current(),
        backend_availability,
        reason: "round-trip fixture".into(),
        native_results: vec![record.clone(); phase_count],
        interpreter_results: vec![record; phase_count],
        equivalent: true,
        mismatch_classes: Vec::new(),
        reproduced_mismatch: None,
    }
}

fn serialize_report(report: &ReplayReport) -> String {
    serde_json::to_string_pretty(report).expect("replay report is serializable")
}

fn decode_replay_report(source: &str) -> Result<ReplayReport, String> {
    if source.len() > 128 * 1024 {
        return Err("saved replay report exceeds the 128 KiB limit".into());
    }
    let report: ReplayReport = serde_json::from_str(source)
        .map_err(|error| format!("invalid replay report JSON: {error}"))?;
    if report.report_version != REPORT_VERSION {
        return Err(format!(
            "unsupported replay report version {}",
            report.report_version
        ));
    }
    if report.native_results.is_empty()
        || report.native_results.len() != report.interpreter_results.len()
    {
        return Err("saved report has missing or unpaired backend snapshots".into());
    }
    let expected_phases = if report.case_envelope.protocol.continuation.is_some() {
        2
    } else {
        1
    };
    if report.native_results.len() != expected_phases {
        return Err(format!(
            "saved report has {} result phases for a protocol requiring {expected_phases}",
            report.native_results.len()
        ));
    }
    for snapshot in report
        .native_results
        .iter()
        .chain(&report.interpreter_results)
    {
        decode_hex_array::<RAM_LEN>(&snapshot.ram_hex)
            .map_err(|error| format!("invalid saved result RAM: {error}"))?;
    }
    report.case_envelope.validate()?;
    let actual_classes = report
        .native_results
        .iter()
        .zip(&report.interpreter_results)
        .flat_map(|(native, interpreter)| record_mismatch_classes(native, interpreter))
        .collect::<Vec<_>>();
    if actual_classes != report.mismatch_classes || report.equivalent != actual_classes.is_empty() {
        return Err("saved mismatch classes do not match the stored backend snapshots".into());
    }
    Ok(report)
}

fn record_mismatch_classes(
    native: &ObservationRecord,
    interpreter: &ObservationRecord,
) -> Vec<String> {
    let mut classes = Vec::new();
    for (index, (left, right)) in native.gpr.iter().zip(&interpreter.gpr).enumerate() {
        if left != right {
            classes.push(format!("gpr[{index}]"));
        }
    }
    if native.rflags != interpreter.rflags {
        classes.push("rflags".into());
    }
    if native.rip != interpreter.rip {
        classes.push("rip".into());
    }
    let native_ram = decode_hex_vec(&native.ram_hex).unwrap_or_default();
    let interpreter_ram = decode_hex_vec(&interpreter.ram_hex).unwrap_or_default();
    if native_ram.len() != interpreter_ram.len() {
        classes.push("ram_size".into());
    }
    for (index, (left, right)) in native_ram.iter().zip(&interpreter_ram).enumerate() {
        if left != right {
            classes.push(format!("ram[{index}]"));
        }
    }
    if native.callbacks != interpreter.callbacks {
        classes.push("callbacks".into());
    }
    if native.memory_accesses != interpreter.memory_accesses {
        classes.push("memory_accesses".into());
    }
    if native.status != interpreter.status {
        classes.push("exit".into());
    }
    if native.remaining_budget != interpreter.remaining_budget {
        classes.push("remaining_budget".into());
    }
    if native.retired != interpreter.retired {
        classes.push("retirement".into());
    }
    if classes.is_empty() && native.state_debug != interpreter.state_debug {
        classes.push("auxiliary_cpu_state".into());
    }
    classes
}

fn decode_hex_vec(value: &str) -> Result<Vec<u8>, String> {
    if !value.is_ascii() || value.len() % 2 != 0 {
        return Err("hex string has odd length".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| format!("invalid hex byte at offset {}", index / 2))
        })
        .collect()
}

fn execute_replay_envelope(
    envelope: &CaseEnvelope,
) -> Result<(Vec<Observation>, Vec<Observation>), String> {
    let input = envelope.validate()?;
    if !native_backend_available() {
        return Err(
            "native JIT replay backend unavailable on this host (requires macOS ARM64)".into(),
        );
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let _guard = JIT_TEST_LOCK
        .lock()
        .map_err(|_| "native JIT test lock is poisoned".to_owned())?;

    let source = envelope.sequence.bytes();
    if hex(&source) != envelope.source_bytes_hex {
        return Err("source byte check failed before native entry".into());
    }
    let block = od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(
        od_jit::block::CodeView::new(ENTRY_RIP, &source)
            .map_err(|error| format!("invalid source code view: {error:?}"))?,
        ENTRY_RIP,
    )
    .map_err(|error| format!("unsupported source bytes: {error:?}"))?;
    let published = PublishedCode::publish_ir_memory_two_site_v2(&block)
        .map_err(|error| format!("native publication unavailable: {error:?}"))?;
    let protocol = &envelope.protocol;
    let fault = protocol
        .fault
        .as_ref()
        .map(|fault| (fault.site_index, fault.failure.into_memory_error()));
    let stop = AtomicU8::new(u8::from(protocol.stop_requested));
    let first_native = native_run(
        &published,
        &input,
        protocol.budget,
        protocol.stop_rip,
        &stop,
        fault.clone(),
    );
    let first_interpreter = interpreter_run(
        &input,
        protocol.budget,
        protocol.stop_rip,
        protocol.stop_requested,
        fault,
    );
    let mut native_results = vec![first_native.clone()];
    let mut interpreter_results = vec![first_interpreter.clone()];
    if protocol.continuation.is_some() {
        native_results.push(native_continue(&published, &input, &first_native));
        interpreter_results.push(interpreter_continue(&input, &first_interpreter));
    }
    Ok((native_results, interpreter_results))
}

fn replay_report_text(source: &str) -> Result<ReplayReport, String> {
    replay_report_text_with_backend(source, native_backend_available())
}

fn replay_report_text_with_backend(
    source: &str,
    current_native_available: bool,
) -> Result<ReplayReport, String> {
    let original = decode_replay_report(source)?;
    if !original.backend_availability.native || !original.backend_availability.interpreter {
        return Err("saved report did not have both execution backends available".into());
    }
    if !current_native_available {
        return Err(
            "native JIT replay backend unavailable on this host (requires macOS ARM64)".into(),
        );
    }
    let (native, interpreter) = execute_replay_envelope(&original.case_envelope)?;
    let classes = actual_mismatch_classes(&native, &interpreter);
    let reproduced_mismatch = original.mismatch_classes == classes;
    Ok(ReplayReport {
        report_version: REPORT_VERSION,
        case: format!("replay:{}", original.case),
        case_envelope: original.case_envelope,
        environment: EnvironmentInfo::current(),
        backend_availability: BackendAvailability::current(),
        reason: "independent replay".into(),
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        equivalent: classes.is_empty(),
        mismatch_classes: classes,
        reproduced_mismatch: Some(reproduced_mismatch),
    })
}

fn actual_mismatch_classes(native: &[Observation], interpreter: &[Observation]) -> Vec<String> {
    actual_mismatch_classes_by_phase(native, interpreter)
        .into_iter()
        .flatten()
        .collect()
}

fn actual_mismatch_classes_by_phase(
    native: &[Observation],
    interpreter: &[Observation],
) -> Vec<Vec<String>> {
    native
        .iter()
        .zip(interpreter)
        .map(|(native, interpreter)| mismatch_classes(native, interpreter))
        .collect()
}

#[derive(Clone, Copy)]
enum ReductionUnit {
    GprGroup(usize),
    RamGroup(usize),
    GprByte(usize, usize),
    RamByte(usize),
    GprBit(usize, u32),
    RamBit(usize, u32),
}

fn reducer_units(sequence: Sequence) -> Vec<ReductionUnit> {
    let mut units = Vec::new();
    for index in 0..16 {
        if !is_address_register(sequence, index) {
            units.push(ReductionUnit::GprGroup(index));
        }
    }
    for offset in (0..RAM_LEN).step_by(8) {
        units.push(ReductionUnit::RamGroup(offset));
    }
    for index in 0..16 {
        if !is_address_register(sequence, index) {
            for byte in 0..8 {
                units.push(ReductionUnit::GprByte(index, byte));
            }
        }
    }
    for offset in 0..RAM_LEN {
        units.push(ReductionUnit::RamByte(offset));
    }
    for index in 0..16 {
        if !is_address_register(sequence, index) {
            for bit in 0..64 {
                units.push(ReductionUnit::GprBit(index, bit));
            }
        }
    }
    for offset in 0..RAM_LEN {
        for bit in 0..8 {
            units.push(ReductionUnit::RamBit(offset, bit));
        }
    }
    units
}

fn is_address_register(sequence: Sequence, index: usize) -> bool {
    index == 3 || (sequence == Sequence::LoadThenStore && index == 2)
}

fn reduction_complexity(envelope: &CaseEnvelope) -> u32 {
    let mut score = envelope
        .initial_cpu
        .gpr
        .iter()
        .enumerate()
        .filter(|(index, _)| !is_address_register(envelope.sequence, *index))
        .map(|(_, value)| value.count_ones())
        .sum::<u32>();
    score += decode_hex_array::<RAM_LEN>(&envelope.initial_ram_hex)
        .expect("validated reducer envelope RAM")
        .iter()
        .map(|byte| u32::from(*byte).count_ones())
        .sum::<u32>();
    score
}

fn reduced_candidate(envelope: &CaseEnvelope, unit: ReductionUnit) -> CaseEnvelope {
    let mut candidate = envelope.clone();
    match unit {
        ReductionUnit::GprGroup(index) => {
            candidate.initial_cpu.gpr[index] = 0;
        }
        ReductionUnit::RamGroup(offset) => {
            let mut ram = decode_hex_array::<RAM_LEN>(&candidate.initial_ram_hex)
                .expect("validated reducer envelope RAM");
            ram[offset..offset + 8].fill(0);
            candidate.initial_ram_hex = hex(&ram);
        }
        ReductionUnit::GprByte(index, byte) => {
            let shift = (byte * 8) as u32;
            let value = candidate.initial_cpu.gpr[index];
            candidate.initial_cpu.gpr[index] = value & !(0xff_u64 << shift);
        }
        ReductionUnit::RamByte(offset) => {
            let mut ram = decode_hex_array::<RAM_LEN>(&candidate.initial_ram_hex)
                .expect("validated reducer envelope RAM");
            ram[offset] = 0;
            candidate.initial_ram_hex = hex(&ram);
        }
        ReductionUnit::GprBit(index, bit) => {
            candidate.initial_cpu.gpr[index] &= !(1_u64 << bit);
        }
        ReductionUnit::RamBit(offset, bit) => {
            let mut ram = decode_hex_array::<RAM_LEN>(&candidate.initial_ram_hex)
                .expect("validated reducer envelope RAM");
            ram[offset] &= !(1_u8 << bit);
            candidate.initial_ram_hex = hex(&ram);
        }
    }
    candidate
}

// This deliberately test-only corruption is used to validate the reducer's
// acceptance and rejection path. It is not called by S3-34b replay or runtime code.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn reducer_test_run(envelope: &CaseEnvelope) -> Result<ReducerRun, String> {
    let (mut native, interpreter) = execute_replay_envelope(envelope)?;
    if envelope.initial_cpu.gpr[11] != 0
        && let Some(first) = native.first_mut()
    {
        first.state.gpr[10] ^= 1;
    }
    Ok(ReducerRun {
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        mismatch_classes_by_phase: actual_mismatch_classes_by_phase(&native, &interpreter),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
// Test-only probe keeps the synthetic mismatch when GPR[11] is cleared, so
// the shared reducer core proves it has no marker-register special case.
fn reducer_marker_independent_test_run(envelope: &CaseEnvelope) -> Result<ReducerRun, String> {
    let (mut native, interpreter) = execute_replay_envelope(envelope)?;
    if let Some(first) = native.first_mut() {
        first.state.gpr[10] ^= 1;
    }
    Ok(ReducerRun {
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        mismatch_classes_by_phase: actual_mismatch_classes_by_phase(&native, &interpreter),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn reducer_final_phase_test_run(envelope: &CaseEnvelope) -> Result<ReducerRun, String> {
    let (mut native, interpreter) = execute_replay_envelope(envelope)?;
    if let Some(last) = native.last_mut() {
        last.state.gpr[10] ^= 1;
    }
    Ok(ReducerRun {
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        mismatch_classes_by_phase: actual_mismatch_classes_by_phase(&native, &interpreter),
    })
}

fn reducer_native_run(envelope: &CaseEnvelope) -> Result<ReducerRun, String> {
    let (native, interpreter) = execute_replay_envelope(envelope)?;
    Ok(ReducerRun {
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        mismatch_classes_by_phase: actual_mismatch_classes_by_phase(&native, &interpreter),
    })
}

fn reducer_load_pair_site_test_run(envelope: &CaseEnvelope) -> Result<ReducerRun, String> {
    if envelope.sequence != Sequence::LoadThenLoad
        || envelope
            .protocol
            .fault
            .as_ref()
            .is_none_or(|fault| fault.site_index != 1 || fault.operation != MemoryAccess::Read)
        || envelope.first_address != envelope.second_address
        || ![0, 1, 7, 111, 112, 120].contains(&envelope.first_address)
    {
        return Err("reducer candidate lost LoadThenLoad site/boundary constraints".into());
    }
    let input = envelope.validate()?;
    let base = Observation {
        state: initial_state(&input, ENTRY_RIP + 3),
        ram: input.ram,
        callbacks: vec!["read", "read"],
        memory_accesses: vec![
            format!("read@{:#x}", input.first_address),
            format!("read@{:#x}", input.first_address),
        ],
        memory_error: None,
        status: "GuestFault(ArchitecturalFault { vector: 14 })".into(),
        remaining_budget: 1,
        retired: 1,
    };
    let mut native = base.clone();
    native.state.gpr[10] ^= 1;
    let classes = actual_mismatch_classes(&[native.clone()], &[base.clone()]);
    Ok(ReducerRun {
        native_results: vec![native.to_record()],
        interpreter_results: vec![base.to_record()],
        mismatch_classes_by_phase: vec![classes],
    })
}

fn reduce_report_text(
    source: &str,
    attempt_limit: usize,
    current_native_available: bool,
) -> Result<ReductionReport, String> {
    reduce_report_text_with_probe(
        source,
        attempt_limit,
        current_native_available,
        reducer_native_run,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn reduce_report_text_with_test_corruption(
    source: &str,
    attempt_limit: usize,
    current_native_available: bool,
) -> Result<ReductionReport, String> {
    reduce_report_text_with_probe(
        source,
        attempt_limit,
        current_native_available,
        reducer_test_run,
    )
}

fn reduce_report_text_with_probe(
    source: &str,
    attempt_limit: usize,
    current_native_available: bool,
    run_candidate: fn(&CaseEnvelope) -> Result<ReducerRun, String>,
) -> Result<ReductionReport, String> {
    if attempt_limit > MAX_REDUCER_ATTEMPTS {
        return Err(format!(
            "reducer attempt limit {attempt_limit} exceeds maximum {MAX_REDUCER_ATTEMPTS}"
        ));
    }
    let original_report = decode_replay_report(source)?;
    if !original_report.backend_availability.native
        || !original_report.backend_availability.interpreter
    {
        return Err("saved report did not have both execution backends available".into());
    }
    if !current_native_available {
        return Err("native JIT reducer unavailable on this host (requires macOS ARM64)".into());
    }
    let original = original_report.case_envelope;
    let classes_by_phase = original_report
        .native_results
        .iter()
        .zip(&original_report.interpreter_results)
        .map(|(native, interpreter)| record_mismatch_classes(native, interpreter))
        .collect::<Vec<_>>();
    let first = run_candidate(&original)?;
    let second = run_candidate(&original)?;
    let mut current = original.clone();
    let mut reduced_run = second.clone();
    let mut attempts = 0;
    let mut stop_reason = if classes_by_phase.iter().all(Vec::is_empty) {
        "original_report_has_no_mismatch".to_owned()
    } else if first.mismatch_classes_by_phase != second.mismatch_classes_by_phase {
        "original_replay_unstable".to_owned()
    } else if first.mismatch_classes_by_phase != classes_by_phase {
        "original_mismatch_not_reproducible".to_owned()
    } else {
        "fixed_point".to_owned()
    };

    if stop_reason == "fixed_point" {
        'units: for unit in reducer_units(original.sequence) {
            if attempts >= attempt_limit {
                stop_reason = "attempt_limit".into();
                break;
            }
            let candidate = reduced_candidate(&current, unit);
            if reduction_complexity(&candidate) >= reduction_complexity(&current) {
                continue;
            }
            attempts += 1;
            let run = run_candidate(&candidate)?;
            if run.mismatch_classes_by_phase == classes_by_phase {
                current = candidate;
                reduced_run = run;
            }
            if attempts >= attempt_limit {
                stop_reason = "attempt_limit".into();
                break 'units;
            }
        }
    }

    Ok(ReductionReport {
        reducer_version: 1,
        environment: EnvironmentInfo::current(),
        backend_availability: BackendAvailability::current(),
        original_envelope: original,
        reduced_envelope: current,
        original_mismatch_classes_by_phase: classes_by_phase.clone(),
        reduced_mismatch_classes_by_phase: reduced_run.mismatch_classes_by_phase.clone(),
        initial_replays: vec![first, second],
        reduced_run,
        attempts,
        attempt_limit,
        stop_reason,
    })
}

fn reduce_saved_case(path: &Path, attempt_limit: usize) -> Result<PathBuf, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read saved case {}: {error}", path.display()))?;
    let report = reduce_report_text(&source, attempt_limit, native_backend_available())?;
    write_reduction_report(path, &report)
}

fn write_reduction_report(path: &Path, report: &ReductionReport) -> Result<PathBuf, String> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "saved case path has no UTF-8 file stem".to_owned())?;
    let output = path.with_file_name(format!("{stem}.reduced-{}.json", std::process::id()));
    if output == path {
        return Err("reducer output path aliases the original report".into());
    }
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("cannot serialize reduction report: {error}"))?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| {
            format!(
                "cannot create reduction report {}: {error}",
                output.display()
            )
        })?;
    file.write_all(&bytes).map_err(|error| {
        format!(
            "cannot write reduction report {}: {error}",
            output.display()
        )
    })?;
    Ok(output)
}

fn replay_saved_case(path: &Path) -> Result<PathBuf, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read saved case {}: {error}", path.display()))?;
    let replay = replay_report_text(&source)?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "saved case path has no UTF-8 file stem".to_owned())?;
    let output = path.with_file_name(format!("{stem}.replay-{}.json", std::process::id()));
    if output == path {
        return Err("replay output path aliases the original report".into());
    }
    let bytes = serde_json::to_vec_pretty(&replay)
        .map_err(|error| format!("cannot serialize replay report: {error}"))?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| format!("cannot create replay report {}: {error}", output.display()))?;
    file.write_all(&bytes)
        .map_err(|error| format!("cannot write replay report {}: {error}", output.display()))?;
    Ok(output)
}

fn replay_path_from_env(value: Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    value.map(PathBuf::from).ok_or_else(|| {
        "OD_JIT_REPLAY_CASE is required; set it to the saved JSON report path".into()
    })
}

fn reducer_path_from_env(value: Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    value.map(PathBuf::from).ok_or_else(|| {
        "OD_JIT_REDUCE_CASE is required; set it to a saved JSON mismatch report".into()
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn fixed_seed_memory_corpus_matches_interpreter_twice_per_input() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let mut unique = HashSet::new();
    let mut address_relations = [0usize; 3];
    let mut load_then_load_boundaries = [0usize; 6];
    // Publication is process-exclusive. Compile and execute each sequence in
    // separate scopes below, retaining only its immutable IR artifact.
    let blocks: Vec<_> = SEQUENCES
        .into_iter()
        .map(|sequence| {
            let source = sequence.bytes();
            od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(
                od_jit::block::CodeView::new(ENTRY_RIP, &source).unwrap(),
                ENTRY_RIP,
            )
            .unwrap()
        })
        .collect();
    let mut total = 0;
    for ((sequence, block), seed) in SEQUENCES
        .into_iter()
        .zip(blocks.iter())
        .flat_map(|pair| SEEDS.into_iter().map(move |seed| (pair, seed)))
    {
        let _ = sequence;
        let published = PublishedCode::publish_ir_memory_two_site_v2(block).unwrap();
        for case_index in 0..CASES_PER_SEED {
            let input = generated_input(seed, case_index, Some(sequence));
            if sequence == Sequence::LoadThenStore {
                let distance = input.first_address.abs_diff(input.second_address);
                let relation = if distance == 0 {
                    0
                } else if distance < 8 {
                    1
                } else {
                    2
                };
                address_relations[relation] += 1;
            }
            if sequence == Sequence::LoadThenLoad {
                let boundary = [0, 1, 7, 111, 112, 120]
                    .iter()
                    .position(|address| input.first_address == *address)
                    .expect("LoadThenLoad generator must cover each accepted boundary");
                load_then_load_boundaries[boundary] += 1;
                assert_eq!(input.first_address, input.second_address);
            }
            assert!(
                unique.insert(input.clone()),
                "duplicate corpus input: seed={seed:#x} case={case_index}"
            );
            let case_id = total;
            total += 1;
            for replay in 0..2 {
                let stop = AtomicU8::new(0);
                let native = native_run(&published, &input, 2, None, &stop, None);
                let interpreter = interpreter_run(&input, 2, None, false, None);
                assert_matches(seed, case_id * 2 + replay, &input, &native, &interpreter);
                assert_eq!(native.status, "Complete", "seed={seed:#x} case={case_id}");
                assert_eq!(native.retired, 2, "seed={seed:#x} case={case_id}");
                assert_eq!(native.callbacks.len(), 2, "seed={seed:#x} case={case_id}");
                assert_eq!(native.callbacks, expected_order(input.sequence));
                assert_eq!(native.memory_accesses, expected_accesses(&input));
            }
        }
        drop(published);
    }
    assert_eq!(total, 3600);
    assert_eq!(unique.len(), total);
    assert!(address_relations.iter().all(|count| *count > 0));
    assert!(load_then_load_boundaries.iter().all(|count| *count > 0));
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn capacity_one_memory_cache_matches_uncached_dispatch_for_frozen_profiles() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    use od_jit::memory_cache::MemoryPairProfile;

    struct Case {
        seed: u64,
        case_id: usize,
        input: CorpusInput,
        uncached: Observation,
        interpreter: Observation,
    }

    let sequences = [Sequence::StoreThenLoad, Sequence::LoadThenStore];
    let mut unique = HashSet::new();
    let mut cases = Vec::with_capacity(sequences.len() * SEEDS.len() * CASES_PER_SEED);

    // Run the uncached dispatcher first. It drops its one-shot mapping on each
    // return, so no mapping overlaps the later resident-cache phase.
    for sequence in sequences {
        for seed in SEEDS {
            for case_index in 0..CASES_PER_SEED {
                let input = generated_input(seed, case_index, Some(sequence));
                assert!(unique.insert(input.clone()), "duplicate S3-34a input");
                let uncached = uncached_dispatch_run(&input, 2, false, None, None, None);
                let interpreter = interpreter_run(&input, 2, None, false, None);
                let case_id = cases.len();
                assert_matches(seed, case_id, &input, &uncached, &interpreter);
                assert_eq!(uncached.status, "Complete");
                assert_eq!(uncached.retired, 2);
                assert_eq!(uncached.memory_accesses, expected_accesses(&input));
                cases.push(Case {
                    seed,
                    case_id,
                    input,
                    uncached,
                    interpreter,
                });
            }
        }
    }
    assert_eq!(cases.len(), 2 * 3 * 400);
    assert_eq!(unique.len(), cases.len());

    let mut cache = od_jit::memory_cache::MemoryPairCache::new();
    let mut backend = CachedPublishedMemoryPairBackend;
    for case in &cases {
        let (profile, context_generation) = match case.input.sequence {
            Sequence::StoreThenLoad => (MemoryPairProfile::StoreThenLoad, 1),
            Sequence::LoadThenStore => (MemoryPairProfile::LoadThenStore, 2),
            Sequence::LoadThenLoad => panic!("S3-18e must not admit LoadThenLoad"),
        };
        for replay in 0..2 {
            let cached = cached_dispatch_run(
                &mut cache,
                &mut backend,
                profile,
                context_generation,
                &case.input,
                2,
                false,
                None,
                None,
                None,
            );
            let envelope = CaseEnvelope::from_input(
                &case.input,
                case.seed,
                case.case_id * 2 + replay,
                ReplayProtocol::successful_pair(),
            );
            assert_matches_named("capacity_one_cache", &envelope, &cached, &case.uncached);
            assert_matches_named(
                "capacity_one_interpreter",
                &envelope,
                &cached,
                &case.interpreter,
            );
            assert_eq!(cached.callbacks, expected_order(case.input.sequence));
            assert_eq!(cached.memory_accesses, expected_accesses(&case.input));
        }
    }

    let counters = cache.counters();
    assert_eq!(counters.misses, 2);
    assert_eq!(counters.hits, 2 * u64::try_from(cases.len()).unwrap() - 2);
    assert_eq!(counters.successful_compiles, 2);
    assert_eq!(counters.successful_publications, 2);
    assert_eq!(cache.resident_entries(), 1);
    cache.clear();
    assert_eq!(cache.resident_entries(), 0);
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn capacity_one_memory_cache_preserves_control_and_committed_prefix_behavior() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    use od_jit::memory_cache::MemoryPairProfile;

    let input = generated_input(SEEDS[0], 0, Some(Sequence::StoreThenLoad));
    let profile = MemoryPairProfile::StoreThenLoad;
    let mut cache = od_jit::memory_cache::MemoryPairCache::new();
    let mut backend = CachedPublishedMemoryPairBackend;

    let stopped = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        2,
        true,
        None,
        None,
        None,
    );
    assert_eq!(stopped.status, "StopRequested");
    assert!(stopped.callbacks.is_empty());
    let stop_rip = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        2,
        false,
        Some(ENTRY_RIP),
        None,
        None,
    );
    assert_eq!(stop_rip.status, "StopAddress");
    assert!(stop_rip.callbacks.is_empty());
    let no_budget = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        0,
        false,
        None,
        None,
        None,
    );
    assert_eq!(no_budget.status, "BudgetExhausted");
    assert!(no_budget.callbacks.is_empty());
    let pending_before_entry = Arc::new(AtomicBool::new(true));
    let event_pending = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        2,
        false,
        None,
        Some(&pending_before_entry),
        None,
    );
    assert_eq!(event_pending.status, "EventPending");
    assert!(event_pending.callbacks.is_empty());
    assert_eq!(cache.counters().successful_publications, 0);

    // The callback raises an event during a two-site artifact. Both paths must
    // finish that in-flight pair, commit callbacks once, and return at code end.
    let uncached_event = Arc::new(AtomicBool::new(false));
    let uncached = uncached_dispatch_run(&input, 2, false, None, Some(&uncached_event), None);
    assert!(uncached_event.load(std::sync::atomic::Ordering::Acquire));
    let cached_event = Arc::new(AtomicBool::new(false));
    let cached = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        2,
        false,
        None,
        Some(&cached_event),
        None,
    );
    assert!(cached_event.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(compare_observations(&cached, &uncached), Ok(()));
    assert_eq!(cached.callbacks, expected_order(input.sequence));

    cache.clear();
    let uncached_prefix = uncached_dispatch_run(&input, 1, false, None, None, None);
    let cached_prefix = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        1,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&cached_prefix, &uncached_prefix),
        Ok(())
    );
    assert_eq!(cached_prefix.status, "BudgetExhausted");
    assert_eq!(cached_prefix.retired, 1);
    assert_eq!(cached_prefix.callbacks, ["write"]);
    assert_eq!(cached_prefix.state.rip, ENTRY_RIP + 3);
    let interpreter_prefix = interpreter_run(&input, 1, None, false, None);
    assert_eq!(
        compare_observations(&cached_prefix, &interpreter_prefix),
        Ok(())
    );

    // Drop the retained mapping before exercising the existing one-shot
    // dispatcher for continuation from the actual second-instruction RIP.
    cache.clear();
    let resumed = uncached_dispatch_continue(&input, &cached_prefix);
    let interpreter_resumed = interpreter_continue(&input, &interpreter_prefix);
    assert_eq!(compare_observations(&resumed, &interpreter_resumed), Ok(()));
    assert_eq!(resumed.callbacks, ["write", "read"]);
    assert_eq!(resumed.memory_accesses, expected_accesses(&input));

    let uncached_stop = uncached_dispatch_run(&input, 2, false, Some(ENTRY_RIP + 3), None, None);
    let cached_stop = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        2,
        false,
        Some(ENTRY_RIP + 3),
        None,
        None,
    );
    assert_eq!(compare_observations(&cached_stop, &uncached_stop), Ok(()));
    assert_eq!(cached_stop.status, "StopAddress");
    assert_eq!(cached_stop.retired, 1);
    assert_eq!(cached_stop.callbacks, ["write"]);
    cache.clear();

    let second_site_fault = MemoryError::Architectural(ArchitecturalFault {
        vector: 14,
        error_code: Some(2),
        linear_address: Some(input.first_address),
    });
    let uncached_fault = uncached_dispatch_run(
        &input,
        2,
        false,
        None,
        None,
        Some((1, second_site_fault.clone())),
    );
    let cached_fault = cached_dispatch_run(
        &mut cache,
        &mut backend,
        profile,
        1,
        &input,
        2,
        false,
        None,
        None,
        Some((1, second_site_fault.clone())),
    );
    let interpreter_fault = interpreter_run(&input, 2, None, false, Some((1, second_site_fault)));
    assert_eq!(compare_observations(&cached_fault, &uncached_fault), Ok(()));
    assert_eq!(
        compare_observations(&cached_fault, &interpreter_fault),
        Ok(())
    );
    assert_eq!(cached_fault.retired, 1);
    assert_eq!(cached_fault.callbacks, ["write", "read"]);
    cache.clear();
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn capacity_one_memory_cache_republishes_after_fault_and_backend_error() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    use od_jit::memory_cache::MemoryPairProfile;

    let store_first = generated_input(0x5333_3745_0000_0001, 1, Some(Sequence::StoreThenLoad));
    let store_second = generated_input(0x5333_3745_0000_0002, 7, Some(Sequence::StoreThenLoad));
    let load_store = generated_input(0x5333_3745_0000_0003, 2, Some(Sequence::LoadThenStore));
    let fresh_store = generated_input(0x5333_3745_0000_0004, 9, Some(Sequence::StoreThenLoad));
    assert_ne!(store_first.gpr, store_second.gpr);
    assert_ne!(store_first.ram, store_second.ram);

    // Build all uncached/interpreter references before retaining a mapping.
    // Each uncached call drops its one-shot mapping on return.
    let store_first_uncached = uncached_dispatch_run(&store_first, 2, false, None, None, None);
    let store_first_interpreter = interpreter_run(&store_first, 2, None, false, None);
    let store_second_uncached = uncached_dispatch_run(&store_second, 2, false, None, None, None);
    let store_second_interpreter = interpreter_run(&store_second, 2, None, false, None);

    let stop_uncached =
        uncached_dispatch_run(&store_first, 2, false, Some(ENTRY_RIP + 3), None, None);
    let stop_interpreter = interpreter_run(&store_first, 2, Some(ENTRY_RIP + 3), false, None);

    let architectural_fault = MemoryError::Architectural(ArchitecturalFault {
        vector: 14,
        error_code: Some(2),
        linear_address: Some(store_first.first_address),
    });
    let fault_uncached = uncached_dispatch_run(
        &store_first,
        2,
        false,
        None,
        None,
        Some((1, architectural_fault.clone())),
    );
    let fault_interpreter = interpreter_run(
        &store_first,
        2,
        None,
        false,
        Some((1, architectural_fault.clone())),
    );

    let backend_error = MemoryError::Backend("S3-37e second-site write failure".into());
    let backend_error_uncached = uncached_dispatch_run(
        &load_store,
        2,
        false,
        None,
        None,
        Some((1, backend_error.clone())),
    );
    assert!(backend_error_uncached.status.contains("error"));
    let load_store_uncached = uncached_dispatch_run(&load_store, 2, false, None, None, None);
    let load_store_interpreter = interpreter_run(&load_store, 2, None, false, None);
    let fresh_store_uncached = uncached_dispatch_run(&fresh_store, 2, false, None, None, None);
    let fresh_store_interpreter = interpreter_run(&fresh_store, 2, None, false, None);

    let mut cache = od_jit::memory_cache::MemoryPairCache::new();
    let mut backend = CachedPublishedMemoryPairBackend;

    let cold_store = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::StoreThenLoad,
        1,
        &store_first,
        2,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&cold_store, &store_first_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&cold_store, &store_first_interpreter),
        Ok(())
    );
    assert_eq!(cold_store.status, "Complete");
    assert_eq!(cold_store.callbacks, expected_order(store_first.sequence));
    assert_eq!(cold_store.memory_accesses, expected_accesses(&store_first));
    assert_eq!(cache.resident_entries(), 1);
    assert_eq!(cache.counters().hits, 0);
    assert_eq!(cache.counters().misses, 1);
    assert_eq!(cache.counters().successful_compiles, 1);
    assert_eq!(cache.counters().successful_publications, 1);
    assert_eq!(cache.counters().replacements, 0);

    // The same immutable key is invoked with different CPU/RAM and newly
    // created stop-control objects; the retained handle must not capture them.
    let same_key_hit = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::StoreThenLoad,
        1,
        &store_second,
        2,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&same_key_hit, &store_second_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&same_key_hit, &store_second_interpreter),
        Ok(())
    );
    assert_eq!(
        same_key_hit.callbacks,
        expected_order(store_second.sequence)
    );
    assert_eq!(
        same_key_hit.memory_accesses,
        expected_accesses(&store_second)
    );
    assert_eq!(cache.counters().hits, 1);
    assert_eq!(cache.counters().misses, 1);
    assert_eq!(cache.counters().replacements, 0);
    assert_eq!(cache.resident_entries(), 1);

    let stopped_after_commit = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::StoreThenLoad,
        1,
        &store_first,
        2,
        false,
        Some(ENTRY_RIP + 3),
        None,
        None,
    );
    assert_eq!(
        compare_observations(&stopped_after_commit, &stop_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&stopped_after_commit, &stop_interpreter),
        Ok(())
    );
    assert_eq!(stopped_after_commit.status, "StopAddress");
    assert_eq!(stopped_after_commit.retired, 1);
    assert_eq!(stopped_after_commit.state.rip, ENTRY_RIP + 3);
    assert_eq!(stopped_after_commit.callbacks, ["write"]);
    assert_eq!(cache.resident_entries(), 1);
    assert_eq!(cache.counters().hits, 2);
    assert_eq!(cache.counters().misses, 1);
    assert_eq!(cache.counters().successful_compiles, 1);
    assert_eq!(cache.counters().successful_publications, 1);
    assert_eq!(cache.counters().replacements, 0);

    let architectural_fault_result = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::StoreThenLoad,
        1,
        &store_first,
        2,
        false,
        None,
        None,
        Some((1, architectural_fault)),
    );
    assert_eq!(
        compare_observations(&architectural_fault_result, &fault_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&architectural_fault_result, &fault_interpreter),
        Ok(())
    );
    assert!(architectural_fault_result.status.contains("GuestFault"));
    assert_eq!(architectural_fault_result.retired, 1);
    assert_eq!(architectural_fault_result.state.rip, ENTRY_RIP + 3);
    assert_eq!(architectural_fault_result.callbacks, ["write", "read"]);
    assert_eq!(
        architectural_fault_result.memory_accesses,
        expected_accesses(&store_first)
    );
    assert_eq!(cache.resident_entries(), 1);
    assert_eq!(cache.counters().hits, 3);
    assert_eq!(cache.counters().misses, 1);
    assert_eq!(cache.counters().successful_publications, 1);
    assert_eq!(cache.counters().replacements, 0);

    // The same profile and context generation with a new executable
    // dependency version changes the key while the old mapping is resident.
    // ensure_entry must release it before publishing and running the new one.
    let replacement = cached_dispatch_run_with_dependency_version(
        &mut cache,
        &mut backend,
        MemoryPairProfile::StoreThenLoad,
        1,
        2,
        &store_second,
        2,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&replacement, &store_second_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&replacement, &store_second_interpreter),
        Ok(())
    );
    assert_eq!(replacement.callbacks, expected_order(store_second.sequence));
    assert_eq!(
        replacement.memory_accesses,
        expected_accesses(&store_second)
    );
    assert_eq!(cache.resident_entries(), 1);
    assert_eq!(cache.counters().hits, 3);
    assert_eq!(cache.counters().misses, 2);
    assert_eq!(cache.counters().successful_compiles, 2);
    assert_eq!(cache.counters().successful_publications, 2);
    assert_eq!(cache.counters().replacements, 1);

    // STOP and the architectural fault both retained an entry above. The
    // explicit clear releases the replacement mapping before a context switch.
    cache.clear();
    assert_eq!(cache.resident_entries(), 0);
    assert_eq!(cache.counters().replacements, 1);

    let other_profile = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::LoadThenStore,
        2,
        &load_store,
        2,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&other_profile, &load_store_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&other_profile, &load_store_interpreter),
        Ok(())
    );
    assert_eq!(other_profile.callbacks, expected_order(load_store.sequence));
    assert_eq!(
        other_profile.memory_accesses,
        expected_accesses(&load_store)
    );
    assert_eq!(cache.resident_entries(), 1);
    assert_eq!(cache.counters().misses, 3);
    assert_eq!(cache.counters().successful_compiles, 3);
    assert_eq!(cache.counters().successful_publications, 3);
    assert_eq!(cache.counters().replacements, 1);

    let failed_second_site = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::LoadThenStore,
        2,
        &load_store,
        2,
        false,
        None,
        None,
        Some((1, backend_error.clone())),
    );
    assert!(failed_second_site.status.contains("error"));
    assert!(failed_second_site.status.contains("Backend"));
    assert!(failed_second_site.status.contains("MemoryError"));
    let backend_error_payload = format!(
        "kind: {}, address: 0, length: 0",
        od_jit::memory_bridge::MEMORY_ERROR_BACKEND
    );
    assert!(
        failed_second_site.status.contains(&backend_error_payload),
        "cached backend error payload: {}",
        failed_second_site.status
    );
    assert!(backend_error_uncached.status.contains("MemoryError"));
    assert!(
        backend_error_uncached
            .status
            .contains(&backend_error_payload)
    );
    assert!(backend_error_uncached.status.contains("error"));
    assert_eq!(failed_second_site.memory_error, Some(backend_error.clone()));
    assert_eq!(backend_error_uncached.memory_error, Some(backend_error));

    let mut normalized_cached_error = failed_second_site.clone();
    let mut normalized_uncached_error = backend_error_uncached.clone();
    normalized_cached_error.status = "BackendError".into();
    normalized_uncached_error.status = "BackendError".into();
    assert_eq!(
        compare_observations(&normalized_cached_error, &normalized_uncached_error),
        Ok(())
    );
    assert_eq!(failed_second_site.state.rip, ENTRY_RIP + 3);
    assert_eq!(
        failed_second_site.callbacks,
        expected_order(load_store.sequence)
    );
    assert_eq!(
        failed_second_site.memory_accesses,
        expected_accesses(&load_store)
    );
    assert_eq!(failed_second_site.ram, load_store.ram);
    assert_eq!(cache.resident_entries(), 0);
    assert_eq!(cache.counters().hits, 4);
    assert_eq!(cache.counters().misses, 3);
    assert_eq!(cache.counters().successful_compiles, 3);
    assert_eq!(cache.counters().successful_publications, 3);
    assert_eq!(cache.counters().replacements, 1);

    // A new entry publishes and executes immediately after the backend error;
    // successful publication proves the previous MAP_JIT lease was released.
    let after_error = cached_dispatch_run(
        &mut cache,
        &mut backend,
        MemoryPairProfile::LoadThenStore,
        2,
        &load_store,
        2,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&after_error, &load_store_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&after_error, &load_store_interpreter),
        Ok(())
    );
    assert_eq!(cache.resident_entries(), 1);
    assert_eq!(cache.counters().hits, 4);
    assert_eq!(cache.counters().misses, 4);
    assert_eq!(cache.counters().successful_compiles, 4);
    assert_eq!(cache.counters().successful_publications, 4);
    assert_eq!(cache.counters().replacements, 1);

    // Dropping the cache releases the retained mapping; a fresh cache instance
    // can publish and execute a different invocation without stale pointers.
    assert_eq!(cache.counters().replacements, 1);
    drop(cache);
    let mut fresh_cache = od_jit::memory_cache::MemoryPairCache::new();
    let fresh_backend = &mut backend;
    let fresh_instance = cached_dispatch_run(
        &mut fresh_cache,
        fresh_backend,
        MemoryPairProfile::StoreThenLoad,
        1,
        &fresh_store,
        2,
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        compare_observations(&fresh_instance, &fresh_store_uncached),
        Ok(())
    );
    assert_eq!(
        compare_observations(&fresh_instance, &fresh_store_interpreter),
        Ok(())
    );
    assert_eq!(
        fresh_instance.callbacks,
        expected_order(fresh_store.sequence)
    );
    assert_eq!(
        fresh_instance.memory_accesses,
        expected_accesses(&fresh_store)
    );
    assert_eq!(fresh_cache.resident_entries(), 1);
    assert_eq!(fresh_cache.counters().hits, 0);
    assert_eq!(fresh_cache.counters().misses, 1);
    assert_eq!(fresh_cache.counters().successful_compiles, 1);
    assert_eq!(fresh_cache.counters().successful_publications, 1);
    assert_eq!(fresh_cache.counters().replacements, 0);
    fresh_cache.clear();
    assert_eq!(fresh_cache.resident_entries(), 0);
}

fn expected_order(sequence: Sequence) -> Vec<&'static str> {
    match sequence {
        Sequence::StoreThenLoad => vec!["write", "read"],
        Sequence::LoadThenStore => vec!["read", "write"],
        Sequence::LoadThenLoad => vec!["read", "read"],
    }
}

fn expected_accesses(input: &CorpusInput) -> Vec<String> {
    let addresses = match input.sequence {
        Sequence::StoreThenLoad | Sequence::LoadThenLoad => {
            [input.first_address, input.first_address]
        }
        Sequence::LoadThenStore => [input.first_address, input.second_address],
    };
    expected_order(input.sequence)
        .into_iter()
        .zip(addresses)
        .map(|(operation, address)| format!("{operation}@{address:#x}"))
        .collect()
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn named_fault_stop_budget_and_continuation_cases_are_separate() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let seed = 0x5343_3441_ffff_0001;
    for sequence in SEQUENCES {
        let case_id = sequence as usize;
        let input = generated_input(seed, case_id, Some(sequence));
        let source = sequence.bytes();
        let block = od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(
            od_jit::block::CodeView::new(ENTRY_RIP, &source).unwrap(),
            ENTRY_RIP,
        )
        .unwrap();
        let published = PublishedCode::publish_ir_memory_two_site_v2(&block).unwrap();
        let make_envelope = |protocol| CaseEnvelope::from_input(&input, seed, case_id, protocol);

        for site_index in [0, 1] {
            let fault_address = if site_index == 0 {
                input.first_address
            } else {
                input.second_address
            };
            let fault = MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(2),
                linear_address: Some(fault_address),
            });
            let stop = AtomicU8::new(0);
            let native_fault = native_run(
                &published,
                &input,
                2,
                None,
                &stop,
                Some((site_index, fault.clone())),
            );
            let interp_fault = interpreter_run(&input, 2, None, false, Some((site_index, fault)));
            let fault_envelope = make_envelope(ReplayProtocol {
                budget: 2,
                stop_requested: false,
                stop_rip: None,
                fault: Some(FaultInjection {
                    site_index,
                    operation: sequence.site_operation(site_index),
                    failure: InjectedFailure::Architectural {
                        vector: 14,
                        error_code: Some(2),
                        linear_address: Some(fault_address),
                    },
                }),
                continuation: None,
            });
            assert_matches_named("fault", &fault_envelope, &native_fault, &interp_fault);
            let expected_calls = expected_order(sequence)
                .into_iter()
                .take(usize::from(site_index) + 1)
                .collect::<Vec<_>>();
            assert_named_condition(
                "fault",
                &fault_envelope,
                &native_fault,
                &interp_fault,
                native_fault.status.contains("GuestFault")
                    && interp_fault.status.contains("GuestFault")
                    && native_fault.callbacks == expected_calls
                    && native_fault.memory_accesses
                        == expected_accesses(&input)[..usize::from(site_index) + 1]
                    && native_fault.retired == u64::from(site_index)
                    && native_fault.state.rip == ENTRY_RIP + u64::from(site_index) * 3,
                "expected a precise architectural guest fault at the selected site",
            );
        }

        let requested = AtomicU8::new(1);
        let native_stop = native_run(&published, &input, 2, None, &requested, None);
        let interp_stop = interpreter_run(&input, 2, None, true, None);
        let stop_envelope = make_envelope(ReplayProtocol {
            budget: 2,
            stop_requested: true,
            stop_rip: None,
            fault: None,
            continuation: None,
        });
        assert_matches_named("stop", &stop_envelope, &native_stop, &interp_stop);
        assert_named_condition(
            "stop",
            &stop_envelope,
            &native_stop,
            &interp_stop,
            native_stop.status == "StopRequested" && interp_stop.status == "StopRequested",
            "expected a pre-entry STOP exit",
        );

        let no_stop = AtomicU8::new(0);
        let native_budget = native_run(&published, &input, 1, None, &no_stop, None);
        let interp_budget = interpreter_run(&input, 1, None, false, None);
        let budget_envelope = make_envelope(ReplayProtocol {
            budget: 1,
            stop_requested: false,
            stop_rip: None,
            fault: None,
            continuation: None,
        });
        assert_matches_named(
            "budget_before_continuation",
            &budget_envelope,
            &native_budget,
            &interp_budget,
        );
        assert_named_condition(
            "budget_before_continuation",
            &budget_envelope,
            &native_budget,
            &interp_budget,
            native_budget.status == "BudgetExhausted"
                && interp_budget.status == "BudgetExhausted"
                && native_budget.retired == 1
                && interp_budget.retired == 1
                && native_budget.state.rip == ENTRY_RIP + 3
                && interp_budget.state.rip == ENTRY_RIP + 3,
            "expected one committed instruction before budget exhaustion",
        );

        let native_continuation = native_continue(&published, &input, &native_budget);
        let interp_continuation = interpreter_continue(&input, &interp_budget);
        let native_phases = [native_budget.clone(), native_continuation.clone()];
        let interpreter_phases = [interp_budget.clone(), interp_continuation.clone()];
        let continuation_envelope = make_envelope(ReplayProtocol {
            budget: 1,
            stop_requested: false,
            stop_rip: None,
            fault: None,
            continuation: Some(ContinuationProtocol {
                first_budget: 1,
                resume_instruction_index: 1,
                continuation_budget: 1,
            }),
        });
        assert_matches_named_phases(
            "continuation_from_committed_snapshot",
            &continuation_envelope,
            &native_phases,
            &interpreter_phases,
        );
        assert_named_condition_phases(
            "continuation_from_committed_snapshot",
            &continuation_envelope,
            &native_phases,
            &interpreter_phases,
            native_continuation.status == "Complete"
                && interp_continuation.status == "Complete"
                && native_continuation.retired == 1
                && interp_continuation.retired == 1
                && native_continuation.callbacks == expected_order(sequence)
                && native_continuation.memory_accesses == expected_accesses(&input),
            "expected continuation to retire the second instruction",
        );
        drop(published);
    }
}

#[test]
fn comparator_rejects_corruption_and_serializes_named_diagnostics() {
    let input = generated_input(SEEDS[0], 0, Some(Sequence::StoreThenLoad));
    let base = Observation {
        state: initial_state(&input, ENTRY_RIP + 6),
        ram: input.ram,
        callbacks: expected_order(input.sequence),
        memory_accesses: expected_accesses(&input),
        memory_error: None,
        status: "Complete".into(),
        remaining_budget: 0,
        retired: 2,
    };
    let mut wrong = base.clone();
    wrong.state.gpr[0] ^= 1;
    assert_eq!(mismatch_classes(&base, &wrong), ["gpr[0]"]);
    assert!(compare_observations(&base, &wrong).is_err(), "wrong GPR");
    let mut wrong = base.clone();
    wrong.state.rip += 1;
    assert_eq!(mismatch_classes(&base, &wrong), ["rip"]);
    assert!(compare_observations(&base, &wrong).is_err(), "wrong RIP");
    let mut wrong = base.clone();
    wrong.ram[17] ^= 1;
    assert_eq!(mismatch_classes(&base, &wrong), ["ram[17]"]);
    assert!(
        compare_observations(&base, &wrong).is_err(),
        "wrong memory byte"
    );
    let mut wrong = base.clone();
    wrong.retired -= 1;
    assert_eq!(mismatch_classes(&base, &wrong), ["retirement"]);
    assert!(
        compare_observations(&base, &wrong).is_err(),
        "wrong retirement"
    );
    let mut missing_fault = base.clone();
    missing_fault.status = "GuestFault(ArchitecturalFault { vector: 14 })".into();
    assert_eq!(mismatch_classes(&base, &missing_fault), ["exit"]);
    assert!(
        compare_observations(&base, &missing_fault).is_err(),
        "missing fault"
    );

    let load_then_load = generated_input(SEEDS[0], 2, Some(Sequence::LoadThenLoad));
    let read_pair = Observation {
        state: initial_state(&load_then_load, ENTRY_RIP + 6),
        ram: load_then_load.ram,
        callbacks: vec!["read", "read"],
        memory_accesses: vec![
            format!("read@{:#x}", load_then_load.first_address),
            format!("read@{:#x}", load_then_load.first_address),
        ],
        memory_error: None,
        status: "Complete".into(),
        remaining_budget: 0,
        retired: 2,
    };
    let mut missing_second_read = read_pair.clone();
    missing_second_read.callbacks.pop();
    assert_eq!(missing_second_read.state.gpr, read_pair.state.gpr);
    assert_eq!(
        mismatch_classes(&read_pair, &missing_second_read),
        ["callbacks"]
    );
    assert!(
        compare_observations(&read_pair, &missing_second_read).is_err(),
        "dropping the second read callback must fail even when final GPRs match"
    );
    let mut wrong_read_address = read_pair.clone();
    wrong_read_address.memory_accesses[1] = format!("read@{:#x}", load_then_load.first_address + 1);
    assert_eq!(
        mismatch_classes(&read_pair, &wrong_read_address),
        ["memory_accesses"]
    );
    assert!(compare_observations(&read_pair, &wrong_read_address).is_err());

    let seed = 0x5343_3441_ffff_0001;
    let case_id = 1;
    let input = generated_input(seed, case_id, Some(Sequence::LoadThenStore));
    let native = Observation {
        state: initial_state(&input, ENTRY_RIP + 3),
        ram: input.ram,
        callbacks: vec!["read"],
        memory_accesses: vec![format!("read@{:#x}", input.first_address)],
        memory_error: None,
        status: "BudgetExhausted".into(),
        remaining_budget: 0,
        retired: 1,
    };
    let mut interpreter = native.clone();
    interpreter.status = "Complete".into();

    assert!(compare_observations(&native, &interpreter).is_err());
    let envelope = CaseEnvelope::from_input(
        &input,
        seed,
        case_id,
        ReplayProtocol {
            budget: 1,
            stop_requested: false,
            stop_rip: None,
            fault: None,
            continuation: Some(ContinuationProtocol {
                first_budget: 1,
                resume_instruction_index: 1,
                continuation_budget: 1,
            }),
        },
    );
    let native_phases = [native.clone(), native.clone()];
    let interpreter_phases = [native.clone(), interpreter.clone()];
    let report = mismatch_report(
        "continuation_from_committed_snapshot",
        &envelope,
        &native_phases,
        &interpreter_phases,
        "controlled named mismatch",
    );
    let decoded = decode_replay_report(&report).unwrap();
    assert_eq!(decoded.case, "continuation_from_committed_snapshot");
    assert_eq!(decoded.case_envelope.seed, seed);
    assert_eq!(decoded.case_envelope.case_id, case_id);
    assert_eq!(decoded.case_envelope, envelope);
    assert_eq!(decoded.mismatch_classes, ["exit"]);
    assert_eq!(decoded.native_results.len(), 2);
    assert_eq!(decoded.interpreter_results.len(), 2);
    assert!(
        record_mismatch_classes(&decoded.native_results[0], &decoded.interpreter_results[0])
            .is_empty()
    );
    assert_eq!(decoded.native_results[1].status, "BudgetExhausted");
    assert_eq!(decoded.interpreter_results[1].status, "Complete");
    assert_eq!(
        record_mismatch_classes(&decoded.native_results[1], &decoded.interpreter_results[1]),
        ["exit"]
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn reducer_fixture_report(sequence: Sequence, marker: u64) -> String {
    let seed = 0x5343_3441_ffff_0035;
    let case_id = sequence as usize;
    let mut input = generated_input(seed, case_id, Some(sequence));
    input.gpr[11] = marker;
    input.gpr[0] = 0x1234_5678_9abc_def0;
    let envelope = CaseEnvelope::from_input(
        &input,
        seed,
        case_id,
        ReplayProtocol {
            budget: 1,
            stop_requested: false,
            stop_rip: None,
            fault: None,
            continuation: Some(ContinuationProtocol {
                first_budget: 1,
                resume_instruction_index: 1,
                continuation_budget: 1,
            }),
        },
    );
    let (mut native, interpreter) = execute_replay_envelope(&envelope).unwrap();
    native[0].state.gpr[10] ^= 1;
    let classes = actual_mismatch_classes(&native, &interpreter);
    assert_eq!(classes, ["gpr[10]"]);
    serialize_report(&ReplayReport {
        report_version: REPORT_VERSION,
        case: "test_only_input_dependent_corruption".into(),
        case_envelope: envelope,
        environment: EnvironmentInfo::current(),
        backend_availability: BackendAvailability::current(),
        reason: "test-only reducer self-test corruption".into(),
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        equivalent: false,
        mismatch_classes: classes,
        reproduced_mismatch: None,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn phase_moving_test_probe(envelope: &CaseEnvelope) -> Result<ReducerRun, String> {
    let input = envelope.validate()?;
    let mut native = Vec::new();
    let mut interpreter = Vec::new();
    for _ in 0..2 {
        let base = Observation {
            state: initial_state(&input, ENTRY_RIP + 3),
            ram: input.ram,
            callbacks: Vec::new(),
            memory_accesses: Vec::new(),
            memory_error: None,
            status: "Complete".into(),
            remaining_budget: 0,
            retired: 1,
        };
        native.push(base.clone());
        interpreter.push(base);
    }
    let mismatch_phase = usize::from(envelope.initial_cpu.gpr[0] == 0);
    native[mismatch_phase].state.gpr[10] ^= 1;
    Ok(ReducerRun {
        native_results: native.iter().map(Observation::to_record).collect(),
        interpreter_results: interpreter.iter().map(Observation::to_record).collect(),
        mismatch_classes_by_phase: actual_mismatch_classes_by_phase(&native, &interpreter),
    })
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn bounded_reducer_is_deterministic_preserves_class_and_saves_report() {
    for sequence in SEQUENCES {
        let original_text = reducer_fixture_report(sequence, 0xfeed_beef_0102_0304);
        let original = decode_replay_report(&original_text).unwrap();
        let first = reduce_report_text_with_test_corruption(&original_text, 12, true).unwrap();
        let second = reduce_report_text_with_test_corruption(&original_text, 12, true).unwrap();
        assert_eq!(first, second, "reduction must be deterministic");
        assert_eq!(first.initial_replays.len(), 2);
        assert!(first.attempts <= first.attempt_limit);
        assert_eq!(first.stop_reason, "attempt_limit");
        assert_eq!(first.original_envelope, original.case_envelope);
        assert_eq!(
            first.original_mismatch_classes_by_phase,
            [vec![String::from("gpr[10]")], vec![]]
        );
        assert_eq!(
            first.reduced_mismatch_classes_by_phase,
            first.original_mismatch_classes_by_phase
        );
        assert_eq!(
            first.reduced_run.mismatch_classes_by_phase,
            first.original_mismatch_classes_by_phase
        );
        assert!(
            reduction_complexity(&first.reduced_envelope)
                < reduction_complexity(&first.original_envelope)
        );
        assert_eq!(
            first.reduced_envelope.sequence,
            first.original_envelope.sequence
        );
        assert_eq!(
            first.reduced_envelope.source_bytes_hex,
            first.original_envelope.source_bytes_hex
        );
        assert_eq!(
            first.reduced_envelope.first_address,
            first.original_envelope.first_address
        );
        assert_eq!(
            first.reduced_envelope.second_address,
            first.original_envelope.second_address
        );
        assert_eq!(
            first.reduced_envelope.protocol,
            first.original_envelope.protocol
        );
        assert_eq!(
            first.reduced_envelope.initial_cpu.gpr[11], first.original_envelope.initial_cpu.gpr[11],
            "marker-clearing candidate loses its synthetic mismatch and must be rejected"
        );
        let cleared_marker =
            reduced_candidate(&first.original_envelope, ReductionUnit::GprGroup(11));
        assert_eq!(cleared_marker.initial_cpu.gpr[11], 0);
        assert!(
            reduction_complexity(&cleared_marker) < reduction_complexity(&first.original_envelope)
        );
        assert!(
            reducer_test_run(&cleared_marker)
                .unwrap()
                .mismatch_classes_by_phase
                .iter()
                .all(Vec::is_empty)
        );
        assert_eq!(
            first.reduced_envelope.initial_ram_hex.len(),
            first.original_envelope.initial_ram_hex.len()
        );

        let reduced_saved_report = sample_report(
            first.reduced_envelope.clone(),
            BackendAvailability::current(),
        );
        let replay = replay_report_text(&serialize_report(&reduced_saved_report)).unwrap();
        assert!(replay.equivalent);

        let unique = format!("od-host-reducer-{}-{sequence:?}", std::process::id());
        let directory = std::env::temp_dir().join(unique);
        std::fs::create_dir(&directory).unwrap();
        let original_path = directory.join("original.json");
        std::fs::write(&original_path, &original_text).unwrap();
        let report_path = write_reduction_report(&original_path, &first).unwrap();
        assert_eq!(
            std::fs::read_to_string(&original_path).unwrap(),
            original_text
        );
        let saved: ReductionReport = serde_json::from_slice(&std::fs::read(report_path).unwrap())
            .expect("reducer report must be serializable");
        assert_eq!(saved, first);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn reducer_preserves_load_pair_site_fault_boundary_and_mismatch_class() {
    let seed = 0x5343_3441_ffff_0036;
    let mut input = generated_input(seed, 5, Some(Sequence::LoadThenLoad));
    assert_eq!(input.first_address, 120);
    input.gpr[10] = 0xfeed_beef;
    let envelope = CaseEnvelope::from_input(
        &input,
        seed,
        5,
        ReplayProtocol {
            budget: 2,
            stop_requested: false,
            stop_rip: None,
            fault: Some(FaultInjection {
                site_index: 1,
                operation: MemoryAccess::Read,
                failure: InjectedFailure::Architectural {
                    vector: 14,
                    error_code: Some(2),
                    linear_address: Some(120),
                },
            }),
            continuation: None,
        },
    );
    let run = reducer_load_pair_site_test_run(&envelope).unwrap();
    assert_eq!(
        run.mismatch_classes_by_phase,
        [vec![String::from("gpr[10]")]]
    );
    let report = ReplayReport {
        report_version: REPORT_VERSION,
        case: "test_only_load_pair_site_mismatch".into(),
        case_envelope: envelope.clone(),
        environment: EnvironmentInfo::current(),
        backend_availability: BackendAvailability {
            native: true,
            interpreter: true,
        },
        reason: "synthetic reducer structure self-check".into(),
        native_results: run.native_results,
        interpreter_results: run.interpreter_results,
        equivalent: false,
        mismatch_classes: vec!["gpr[10]".into()],
        reproduced_mismatch: None,
    };
    let reduced = reduce_report_text_with_probe(
        &serialize_report(&report),
        24,
        true,
        reducer_load_pair_site_test_run,
    )
    .unwrap();
    assert_eq!(reduced.reduced_envelope.sequence, Sequence::LoadThenLoad);
    assert_eq!(
        reduced.reduced_envelope.source_bytes_hex,
        envelope.source_bytes_hex
    );
    assert_eq!(reduced.reduced_envelope.first_address, 120);
    assert_eq!(reduced.reduced_envelope.second_address, 120);
    assert_eq!(reduced.reduced_envelope.protocol, envelope.protocol);
    assert_eq!(
        reduced.reduced_mismatch_classes_by_phase,
        [vec![String::from("gpr[10]")]]
    );
}

#[test]
fn reducer_rejects_malformed_and_unavailable_inputs_and_stops_on_nonreproduction() {
    assert!(reduce_report_text("{bad json", 10, false).is_err());
    assert!(
        reduce_report_text("{}", MAX_REDUCER_ATTEMPTS + 1, false)
            .unwrap_err()
            .contains("exceeds maximum")
    );
    let unavailable = sample_report(
        CaseEnvelope::from_input(
            &generated_input(SEEDS[0], 0, Some(Sequence::StoreThenLoad)),
            SEEDS[0],
            0,
            ReplayProtocol::successful_pair(),
        ),
        BackendAvailability::current(),
    );
    assert!(reduce_report_text(&serialize_report(&unavailable), 10, false).is_err());

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let changed_class = reducer_fixture_report(Sequence::StoreThenLoad, 0);
        let stopped = reduce_report_text_with_test_corruption(&changed_class, 10, true).unwrap();
        assert_eq!(stopped.stop_reason, "original_mismatch_not_reproducible");
        assert_eq!(stopped.attempts, 0);
        assert_eq!(stopped.original_envelope, stopped.reduced_envelope);
        assert!(
            stopped
                .initial_replays
                .iter()
                .all(|run| run.mismatch_classes_by_phase.iter().all(Vec::is_empty))
        );

        let mut wrong_original: ReplayReport =
            serde_json::from_str(&reducer_fixture_report(Sequence::LoadThenStore, 1)).unwrap();
        // Update the stored snapshots to retain a valid report whose real replay
        // changes the class to the test-only gpr[10] corruption.
        for (native, interpreter) in wrong_original
            .native_results
            .iter_mut()
            .zip(&wrong_original.interpreter_results)
        {
            native.gpr = interpreter.gpr;
            native.gpr[0] ^= 1;
            native.state_debug = format!(
                "{:?}",
                LongModeState {
                    gpr: native.gpr,
                    rip: native.rip,
                    rflags: native.rflags,
                    ..Default::default()
                }
            );
        }
        wrong_original.mismatch_classes = wrong_original
            .native_results
            .iter()
            .zip(&wrong_original.interpreter_results)
            .flat_map(|(native, interpreter)| record_mismatch_classes(native, interpreter))
            .collect();
        let wrong_original_text = serialize_report(&wrong_original);
        let changed =
            reduce_report_text_with_test_corruption(&wrong_original_text, 10, true).unwrap();
        assert_eq!(changed.stop_reason, "original_mismatch_not_reproducible");
        assert_eq!(changed.attempts, 0);

        let real_replay = reduce_report_text(&wrong_original_text, 10, true).unwrap();
        assert_eq!(
            real_replay.stop_reason,
            "original_mismatch_not_reproducible"
        );
        assert_eq!(real_replay.attempts, 0);
    }
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn reducer_rejects_same_mismatch_moving_to_continuation_phase() {
    let source = reducer_fixture_report(Sequence::StoreThenLoad, 0x1234);
    let original = decode_replay_report(&source).unwrap();
    let envelope = original.case_envelope.clone();
    let moved_candidate = reduced_candidate(&envelope, ReductionUnit::GprGroup(0));
    let original_run = phase_moving_test_probe(&envelope).unwrap();
    let moved_run = phase_moving_test_probe(&moved_candidate).unwrap();
    assert_eq!(
        original_run.mismatch_classes_by_phase,
        [vec![String::from("gpr[10]")], vec![]]
    );
    assert_eq!(
        moved_run.mismatch_classes_by_phase,
        [vec![], vec![String::from("gpr[10]")]]
    );
    assert_eq!(
        original_run
            .mismatch_classes_by_phase
            .iter()
            .flatten()
            .collect::<Vec<_>>(),
        moved_run
            .mismatch_classes_by_phase
            .iter()
            .flatten()
            .collect::<Vec<_>>(),
        "flattened classes are intentionally identical"
    );

    let report = reduce_report_text_with_probe(&source, 1, true, phase_moving_test_probe).unwrap();
    assert_eq!(report.attempts, 1);
    assert_eq!(report.stop_reason, "attempt_limit");
    assert_eq!(report.reduced_envelope, envelope);
    assert_eq!(
        report.reduced_mismatch_classes_by_phase,
        original_run.mismatch_classes_by_phase
    );
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn generic_reducer_can_clear_marker_when_comparator_allows_candidate() {
    let source = reducer_fixture_report(Sequence::StoreThenLoad, 0xfeed_beef);
    let report =
        reduce_report_text_with_probe(&source, 11, true, reducer_marker_independent_test_run)
            .unwrap();
    assert_eq!(report.attempts, 11);
    assert_eq!(report.reduced_envelope.initial_cpu.gpr[11], 0);
    assert_eq!(
        report.reduced_mismatch_classes_by_phase,
        report.original_mismatch_classes_by_phase
    );
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn mismatch_report_preserves_empty_first_and_mismatching_continuation_phases() {
    let mut input = generated_input(0x5343_3441_ffff_0036, 1, Some(Sequence::LoadThenStore));
    input.gpr[11] = 0xfeed_beef;
    input.gpr[0] = 0x1020_3040_5060_7080;
    let envelope = CaseEnvelope::from_input(
        &input,
        0x5343_3441_ffff_0036,
        1,
        ReplayProtocol {
            budget: 1,
            stop_requested: false,
            stop_rip: None,
            fault: None,
            continuation: Some(ContinuationProtocol {
                first_budget: 1,
                resume_instruction_index: 1,
                continuation_budget: 1,
            }),
        },
    );
    let (mut native, interpreter) = execute_replay_envelope(&envelope).unwrap();
    native[1].state.gpr[10] ^= 1;
    let original_text = mismatch_report(
        "test_only_final_phase_corruption",
        &envelope,
        &native,
        &interpreter,
        "test-only continuation observation corruption",
    );
    let decoded = decode_replay_report(&original_text).unwrap();
    assert_eq!(decoded.native_results.len(), 2);
    assert!(
        record_mismatch_classes(&decoded.native_results[0], &decoded.interpreter_results[0])
            .is_empty()
    );
    assert_eq!(
        record_mismatch_classes(&decoded.native_results[1], &decoded.interpreter_results[1]),
        ["gpr[10]"]
    );
    assert_eq!(decoded.mismatch_classes, ["gpr[10]"]);

    let mut missing_phase: serde_json::Value = serde_json::from_str(&original_text).unwrap();
    missing_phase["native_results"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(decode_replay_report(&missing_phase.to_string()).is_err());

    let reduced =
        reduce_report_text_with_probe(&original_text, 1, true, reducer_final_phase_test_run)
            .unwrap();
    assert_eq!(
        reduced.original_mismatch_classes_by_phase,
        [vec![], vec![String::from("gpr[10]")]]
    );
    assert_eq!(
        reduced.initial_replays[0].mismatch_classes_by_phase,
        reduced.original_mismatch_classes_by_phase
    );
    assert_eq!(
        reduced.reduced_mismatch_classes_by_phase,
        reduced.original_mismatch_classes_by_phase
    );
}

#[test]
fn replay_envelope_round_trips_and_rejects_malformed_reports() {
    for sequence in SEQUENCES {
        let input = generated_input(0x5343_3441_ffff_1000, sequence as usize, Some(sequence));
        let envelope = CaseEnvelope::from_input(
            &input,
            0x5343_3441_ffff_1000,
            sequence as usize,
            ReplayProtocol::successful_pair(),
        );
        let original = serialize_report(&sample_report(
            envelope.clone(),
            BackendAvailability {
                native: true,
                interpreter: true,
            },
        ));
        let decoded = decode_replay_report(&original).unwrap();
        assert_eq!(decoded.case_envelope, envelope);
        assert_eq!(decoded.environment.architecture, std::env::consts::ARCH);
        assert!(!decoded.environment.build_revision.is_empty());
        let mut legacy_value: serde_json::Value = serde_json::from_str(&original).unwrap();
        for result in legacy_value["native_results"].as_array_mut().unwrap() {
            result.as_object_mut().unwrap().remove("memory_accesses");
        }
        for result in legacy_value["interpreter_results"].as_array_mut().unwrap() {
            result.as_object_mut().unwrap().remove("memory_accesses");
        }
        let legacy = decode_replay_report(&legacy_value.to_string()).unwrap();
        assert!(legacy.native_results[0].memory_accesses.is_empty());
        assert!(legacy.interpreter_results[0].memory_accesses.is_empty());

        let mutations: [(&str, Box<dyn Fn(&mut serde_json::Value)>); 11] = [
            (
                "unsupported replay report version",
                Box::new(|value| value["report_version"] = serde_json::json!(2)),
            ),
            (
                "unsupported case envelope version",
                Box::new(|value| value["case_envelope"]["version"] = serde_json::json!(2)),
            ),
            (
                "unsupported replay profile",
                Box::new(|value| value["case_envelope"]["profile"] = serde_json::json!("other")),
            ),
            (
                "source bytes do not match",
                Box::new(|value| {
                    value["case_envelope"]["source_bytes_hex"] = serde_json::json!("000000000000")
                }),
            ),
            (
                "entry RIP must be",
                Box::new(|value| {
                    value["case_envelope"]["initial_cpu"]["rip"] = serde_json::json!(ENTRY_RIP + 1)
                }),
            ),
            (
                "invalid RAM snapshot",
                Box::new(|value| {
                    value["case_envelope"]["initial_ram_hex"] = serde_json::json!("00")
                }),
            ),
            (
                "recorded access addresses do not match",
                Box::new(|value| value["case_envelope"]["first_address"] = serde_json::json!(99)),
            ),
            (
                "unsupported instruction budget",
                Box::new(|value| {
                    value["case_envelope"]["protocol"]["budget"] = serde_json::json!(3)
                }),
            ),
            (
                "unsupported stop-RIP",
                Box::new(|value| {
                    value["case_envelope"]["protocol"]["stop_rip"] =
                        serde_json::json!(ENTRY_RIP + 1)
                }),
            ),
            (
                "unknown field",
                Box::new(|value| value["case_envelope"]["unexpected"] = serde_json::json!(true)),
            ),
            (
                "invalid saved result RAM",
                Box::new(|value| value["native_results"][0]["ram_hex"] = serde_json::json!("00")),
            ),
        ];
        for (message, mutate) in mutations {
            let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
            mutate(&mut value);
            let error = decode_replay_report(&value.to_string()).unwrap_err();
            assert!(
                error.contains(message),
                "{error:?} should contain {message:?}"
            );
        }

        let mut bad_trigger: serde_json::Value = serde_json::from_str(&original).unwrap();
        bad_trigger["case_envelope"]["protocol"]["fault"] = serde_json::json!({
            "site_index": 2,
            "operation": "read",
            "failure": {"kind": "architectural", "vector": 14, "error_code": 2, "linear_address": input.first_address}
        });
        let error = decode_replay_report(&bad_trigger.to_string()).unwrap_err();
        assert!(error.contains("fault injection site index must be 0 or 1"));

        let load_then_load =
            generated_input(0x5343_3441_ffff_1000, 2, Some(Sequence::LoadThenLoad));
        let site_one = CaseEnvelope::from_input(
            &load_then_load,
            0x5343_3441_ffff_1000,
            2,
            ReplayProtocol {
                budget: 2,
                stop_requested: false,
                stop_rip: None,
                fault: Some(FaultInjection {
                    site_index: 1,
                    operation: MemoryAccess::Read,
                    failure: InjectedFailure::Architectural {
                        vector: 14,
                        error_code: Some(2),
                        linear_address: Some(load_then_load.first_address),
                    },
                }),
                continuation: None,
            },
        );
        let encoded = serialize_report(&sample_report(
            site_one.clone(),
            BackendAvailability {
                native: true,
                interpreter: true,
            },
        ));
        let decoded = decode_replay_report(&encoded).unwrap();
        assert_eq!(decoded.case_envelope, site_one);
        assert_eq!(
            decoded.case_envelope.sequence.bytes(),
            [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b]
        );

        let mut inconsistent: serde_json::Value = serde_json::from_str(&original).unwrap();
        inconsistent["native_results"][0]["gpr"][0] = serde_json::json!(input.gpr[0] ^ 1);
        let error = decode_replay_report(&inconsistent.to_string()).unwrap_err();
        assert!(error.contains("mismatch classes do not match"));
    }

    let huge = " ".repeat(128 * 1024 + 1);
    assert!(
        decode_replay_report(&huge)
            .unwrap_err()
            .contains("128 KiB limit")
    );
    assert!(
        replay_path_from_env(None)
            .unwrap_err()
            .contains("OD_JIT_REPLAY_CASE is required")
    );
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn replay_success_fault_stop_budget_and_continuation_for_all_sequences() {
    let seed = 0x5343_3441_ffff_2000;
    let mut saved_path = None;
    for sequence in SEQUENCES {
        let case_id = sequence as usize;
        let input = generated_input(seed, case_id, Some(sequence));
        let fault_access = sequence.site_operation(0);
        let scenarios = [
            (
                "success",
                ReplayProtocol::successful_pair(),
                "Complete",
                expected_order(sequence),
                2,
            ),
            (
                "fault",
                ReplayProtocol {
                    budget: 2,
                    stop_requested: false,
                    stop_rip: None,
                    fault: Some(FaultInjection {
                        site_index: 0,
                        operation: fault_access,
                        failure: InjectedFailure::Architectural {
                            vector: 14,
                            error_code: Some(2),
                            linear_address: Some(input.first_address),
                        },
                    }),
                    continuation: None,
                },
                "GuestFault",
                expected_order(sequence)[..1].to_vec(),
                0,
            ),
            (
                "stop",
                ReplayProtocol {
                    budget: 2,
                    stop_requested: true,
                    stop_rip: None,
                    fault: None,
                    continuation: None,
                },
                "StopRequested",
                vec![],
                0,
            ),
            (
                "stop_rip",
                ReplayProtocol {
                    budget: 2,
                    stop_requested: false,
                    stop_rip: Some(ENTRY_RIP + 3),
                    fault: None,
                    continuation: None,
                },
                "StopAddress",
                expected_order(sequence)[..1].to_vec(),
                1,
            ),
            (
                "budget",
                ReplayProtocol {
                    budget: 1,
                    stop_requested: false,
                    stop_rip: None,
                    fault: None,
                    continuation: None,
                },
                "BudgetExhausted",
                expected_order(sequence)[..1].to_vec(),
                1,
            ),
            (
                "continuation",
                ReplayProtocol {
                    budget: 1,
                    stop_requested: false,
                    stop_rip: None,
                    fault: None,
                    continuation: Some(ContinuationProtocol {
                        first_budget: 1,
                        resume_instruction_index: 1,
                        continuation_budget: 1,
                    }),
                },
                "Complete",
                expected_order(sequence),
                1,
            ),
        ];
        for (index, (name, protocol, expected_exit, expected_calls, expected_retired)) in
            scenarios.into_iter().enumerate()
        {
            let envelope = CaseEnvelope::from_input(&input, seed, case_id * 10 + index, protocol);
            let source = serialize_report(&sample_report(
                envelope.clone(),
                BackendAvailability {
                    native: true,
                    interpreter: true,
                },
            ));
            let decoded = decode_replay_report(&source).unwrap();
            assert_eq!(
                decoded.case_envelope, envelope,
                "round trip {name} {sequence:?}"
            );
            let replay = replay_report_text(&source).unwrap();
            assert!(
                replay.reproduced_mismatch == Some(true),
                "replay did not reproduce {name}"
            );
            assert_eq!(
                replay.native_results, replay.interpreter_results,
                "{name} {sequence:?}"
            );
            let final_native = replay.native_results.last().unwrap();
            let final_interpreter = replay.interpreter_results.last().unwrap();
            assert!(
                final_native.status.starts_with(expected_exit),
                "{name}: {}",
                final_native.status
            );
            assert_eq!(
                final_native.callbacks,
                expected_calls
                    .iter()
                    .map(|call| call.to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(final_interpreter.callbacks, final_native.callbacks);
            assert_eq!(
                final_native.memory_accesses,
                expected_accesses(&input)[..expected_calls.len()]
            );
            assert_eq!(final_native.retired, expected_retired);
            if name == "continuation" {
                assert_eq!(replay.native_results.len(), 2);
                assert_eq!(replay.native_results[0].status, "BudgetExhausted");
                assert_eq!(
                    replay.native_results[0].callbacks,
                    expected_order(sequence)[..1].to_vec()
                );
            } else {
                assert_eq!(replay.native_results.len(), 1);
            }
            if name == "success" && sequence == Sequence::StoreThenLoad {
                let path = std::env::temp_dir().join(format!(
                    "od-jit-replay-original-{}.json",
                    std::process::id()
                ));
                std::fs::write(&path, &source).unwrap();
                let output = replay_saved_case(&path).unwrap();
                assert_ne!(path, output);
                assert_eq!(
                    std::fs::read_to_string(&path).unwrap(),
                    source,
                    "original report was overwritten"
                );
                let fresh =
                    decode_replay_report(&std::fs::read_to_string(&output).unwrap()).unwrap();
                assert_eq!(fresh.reproduced_mismatch, Some(true));
                let _ = std::fs::remove_file(path);
                let _ = std::fs::remove_file(output);
                saved_path = Some(true);
            }
        }

        let site_one = CaseEnvelope::from_input(
            &input,
            seed,
            case_id * 10 + 9,
            ReplayProtocol {
                budget: 2,
                stop_requested: false,
                stop_rip: None,
                fault: Some(FaultInjection {
                    site_index: 1,
                    operation: sequence.site_operation(1),
                    failure: InjectedFailure::Architectural {
                        vector: 14,
                        error_code: Some(2),
                        linear_address: Some(input.second_address),
                    },
                }),
                continuation: None,
            },
        );
        let source = serialize_report(&sample_report(
            site_one.clone(),
            BackendAvailability {
                native: true,
                interpreter: true,
            },
        ));
        assert_eq!(
            decode_replay_report(&source).unwrap().case_envelope,
            site_one
        );
        let replay = replay_report_text(&source).unwrap();
        assert_eq!(replay.native_results, replay.interpreter_results);
        assert!(replay.native_results[0].status.starts_with("GuestFault"));
        assert_eq!(replay.native_results[0].callbacks, expected_order(sequence));
        assert_eq!(replay.native_results[0].retired, 1);
    }
    assert_eq!(saved_path, Some(true));
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn saved_mismatch_report_replays_and_preserves_original_classes() {
    let seed = 0x5343_3441_ffff_0034;
    for sequence in SEQUENCES {
        let case_id = sequence as usize;
        let input = generated_input(seed, case_id, Some(sequence));
        let envelope =
            CaseEnvelope::from_input(&input, seed, case_id, ReplayProtocol::successful_pair());
        let native = Observation {
            state: initial_state(&input, ENTRY_RIP + 6),
            ram: input.ram.clone(),
            callbacks: expected_order(sequence),
            memory_accesses: expected_accesses(&input),
            memory_error: None,
            status: "Complete".into(),
            remaining_budget: 0,
            retired: 2,
        };
        let mut corrupted = native.clone();
        corrupted.state.gpr[0] ^= 1;
        let saved_classes = mismatch_classes(&native, &corrupted);
        assert_eq!(saved_classes, ["gpr[0]"]);

        let original_text = mismatch_report(
            "controlled_saved_mismatch",
            &envelope,
            std::slice::from_ref(&native),
            std::slice::from_ref(&corrupted),
            "controlled GPR corruption for replay-path coverage",
        );
        let decoded = decode_replay_report(&original_text).unwrap();
        assert_eq!(decoded.mismatch_classes, saved_classes);
        assert!(!decoded.equivalent);

        let fresh = replay_report_text(&original_text).unwrap();
        assert!(fresh.equivalent, "{fresh:#?}");
        assert_eq!(fresh.mismatch_classes, Vec::<String>::new());
        assert_eq!(fresh.reproduced_mismatch, Some(false));
        assert_eq!(decoded.mismatch_classes, saved_classes);
        assert_eq!(serialize_report(&decoded), original_text);
    }
}

#[test]
fn replay_refuses_to_claim_equivalence_without_native_backend() {
    let input = generated_input(SEEDS[0], 0, Some(Sequence::StoreThenLoad));
    let envelope = CaseEnvelope::from_input(&input, SEEDS[0], 0, ReplayProtocol::successful_pair());
    let source = serialize_report(&sample_report(
        envelope,
        BackendAvailability {
            native: true,
            interpreter: true,
        },
    ));
    let error = replay_report_text_with_backend(&source, false).unwrap_err();
    assert!(error.contains("native JIT replay backend unavailable"));
}

#[test]
#[ignore = "set OD_JIT_REPLAY_CASE to a saved versioned mismatch report"]
fn replay_saved_case_from_environment() {
    let path = replay_path_from_env(std::env::var_os("OD_JIT_REPLAY_CASE"))
        .unwrap_or_else(|error| panic!("{error}"));
    let replay_path =
        replay_saved_case(&path).unwrap_or_else(|error| panic!("case replay failed: {error}"));
    let report: ReplayReport = serde_json::from_str(
        &std::fs::read_to_string(&replay_path).expect("fresh replay report is readable"),
    )
    .expect("fresh replay report is valid JSON");
    assert_eq!(
        report.reproduced_mismatch,
        Some(true),
        "replayed mismatch class differs; see {}",
        replay_path.display()
    );
}

#[test]
#[ignore = "set OD_JIT_REDUCE_CASE to a saved versioned mismatch report"]
fn reduce_saved_case_from_environment() {
    let path = reducer_path_from_env(std::env::var_os("OD_JIT_REDUCE_CASE"))
        .unwrap_or_else(|error| panic!("{error}"));
    let report_path = reduce_saved_case(&path, 512)
        .unwrap_or_else(|error| panic!("case reduction failed: {error}"));
    let report: ReductionReport = serde_json::from_slice(
        &std::fs::read(&report_path).expect("fresh reducer report is readable"),
    )
    .expect("fresh reducer report is valid JSON");
    if report.stop_reason == "original_mismatch_not_reproducible" {
        assert_eq!(
            report.attempts, 0,
            "nonreproducible input must not be reduced"
        );
    } else {
        assert_eq!(
            report.original_mismatch_classes_by_phase,
            report.initial_replays[0].mismatch_classes_by_phase,
            "first fresh replay must reproduce the saved classes"
        );
        assert_eq!(
            report.original_mismatch_classes_by_phase, report.reduced_mismatch_classes_by_phase,
            "accepted reductions must preserve the original mismatch classes"
        );
    }
}
