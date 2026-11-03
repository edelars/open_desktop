#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU8;
use std::time::{SystemTime, UNIX_EPOCH};

use od_host::PublishedCode;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_isa::{LongModeState, OperandSize};
use od_jit::block::CodeView;
use od_jit::memory_load_v2::compile_ir_memory_dword_store_block_v2;
use od_testkit::memory_corpus::{
    DwordStoreCorpusInput, S3_34DM_CASES_PER_SEED, S3_34DM_SEEDS,
    generate_s3_34dm_dword_store_input,
};

const ENTRY_RIP: u64 = 0x9fa0;
const SOURCE: [u8; 4] = [0x67, 0x89, 0x04, 0x24];
const REPLAY_VERSION: u32 = 1;
const REPLAY_PROFILE: &str = "S3-34dm-addr32-rsp-dword-store-v1";
const REPLAY_ENV: &str = "OD_S3_34DN_REPLAY_ARTIFACT";
const REDUCE_ENV: &str = "OD_S3_34DO_REDUCE_ARTIFACT";
const REPLAY_RAM_LAYOUT: &str = "offsets:-4..=7;omit-address-underflow";
const DEFINED_FLAGS_MASK: u64 = 0x8d5;
const MAX_REDUCER_CANDIDATES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
struct WriteCallback {
    address: u64,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct TraceRam {
    bytes: BTreeMap<u64, u8>,
    writes: Vec<WriteCallback>,
}

impl TraceRam {
    fn from_input(input: &DwordStoreCorpusInput) -> Self {
        let mut bytes = BTreeMap::new();
        for offset in -4_i64..=7 {
            if let Some(address) = u64::from(input.address).checked_add_signed(offset) {
                bytes.insert(address, input.ram[(offset + 4) as usize]);
            }
        }
        Self {
            bytes,
            writes: Vec::new(),
        }
    }
}

impl GuestMemory for TraceRam {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        if let Some(offset) = address
            .checked_sub(ENTRY_RIP)
            .and_then(|offset| usize::try_from(offset).ok())
            .filter(|offset| *offset < SOURCE.len())
        {
            return Ok(SOURCE[offset]);
        }
        Err(MemoryError::Unmapped {
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
        for (offset, byte) in destination.iter_mut().enumerate() {
            *byte = *self
                .bytes
                .get(&(address + offset as u64))
                .ok_or(MemoryError::Unmapped {
                    address: address + offset as u64,
                    access: AccessKind::Read,
                })?;
        }
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        _: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        let end = address
            .checked_add(source.len() as u64)
            .ok_or(MemoryError::AddressOverflow {
                address,
                length: source.len(),
            })?;
        if (address..end).any(|at| !self.bytes.contains_key(&at)) {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            });
        }
        self.writes.push(WriteCallback {
            address,
            bytes: source.to_vec(),
        });
        for (offset, byte) in source.iter().copied().enumerate() {
            self.bytes.insert(address + offset as u64, byte);
        }
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation {
    exit: String,
    state: LongModeState,
    ram: BTreeMap<u64, u8>,
    retired_instructions: u64,
    callbacks: Vec<WriteCallback>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReplayArtifact {
    seed: u64,
    index: usize,
    input: DwordStoreCorpusInput,
    interpreter_observation: String,
    jit_observation: String,
    mismatch: String,
}

struct TemporaryArtifactDirectory(PathBuf);

impl Drop for TemporaryArtifactDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplayOutcome {
    NotReproduced,
    Reproduced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReplayRuns {
    interpreter_first: Observation,
    interpreter_second: Observation,
    jit_first: Observation,
    jit_second: Observation,
    mismatch_first: Option<String>,
    mismatch_second: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MismatchField {
    Gpr(usize),
    Ram(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendPair {
    ExpectedInterpreter,
    ExpectedJit,
    InterpreterJit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PairMismatch {
    pair: BackendPair,
    fields: Vec<MismatchField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MismatchSignature {
    pairs: Vec<PairMismatch>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueMutation {
    ZeroGpr(usize),
    ClearGprBit { index: usize, bit: u32 },
    ZeroRam(usize),
    ClearRamBit { index: usize, bit: u8 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValueCandidate {
    mutation: ValueMutation,
    input: DwordStoreCorpusInput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReductionReport {
    outcome: &'static str,
    attempts: usize,
    observations: usize,
    stop_reason: String,
    reduced_artifact: Option<PathBuf>,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    encoded
}

fn unhex(value: &str) -> Result<Vec<u8>, &'static str> {
    if value.len() % 2 != 0 {
        return Err("odd-length hexadecimal field");
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or("invalid hexadecimal field")?;
            let low = (pair[1] as char).to_digit(16).ok_or("invalid hexadecimal field")?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

fn ram_layout(input: &DwordStoreCorpusInput) -> BTreeMap<u64, u8> {
    TraceRam::from_input(input).bytes
}

fn ram_digest(input: &DwordStoreCorpusInput) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for (address, byte) in ram_layout(input) {
        for part in address.to_le_bytes().into_iter().chain([byte]) {
            hash ^= u64::from(part);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

fn address_matches_profile(index: usize, address: u32) -> bool {
    match index % 4 {
        0 => address == 0,
        1 => (1..=0x0fff).contains(&address) && address % 2 == 1,
        2 => (0x1000..=0x8000_0ffe).contains(&address),
        3 => address == [0xffff_fffd, 0xffff_fffe, 0xffff_ffff][index / 4 % 3],
        _ => unreachable!(),
    }
}

fn serialize_replay_artifact(artifact: &ReplayArtifact) -> String {
    let registers = artifact
        .input
        .gpr
        .iter()
        .map(|value| format!("{value:016x}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "version={REPLAY_VERSION}\nprofile={REPLAY_PROFILE}\nrip={ENTRY_RIP:016x}\nsource={}\nseed={:016x}\nindex={}\ngpr={registers}\nflags={:016x}\naddress={:08x}\nram={}\nram_layout={REPLAY_RAM_LAYOUT}\nram_digest={:016x}\ninterpreter_observation={}\njit_observation={}\nmismatch={}\n",
        hex(&SOURCE),
        artifact.seed,
        artifact.index,
        artifact.input.flags,
        artifact.input.address,
        hex(&artifact.input.ram),
        ram_digest(&artifact.input),
        hex(artifact.interpreter_observation.as_bytes()),
        hex(artifact.jit_observation.as_bytes()),
        hex(artifact.mismatch.as_bytes()),
    )
}

fn parse_replay_artifact(contents: &str) -> Result<ReplayArtifact, &'static str> {
    let mut fields = BTreeMap::new();
    for line in contents.lines() {
        let (key, value) = line.split_once('=').ok_or("malformed artifact line")?;
        if fields.insert(key, value).is_some() {
            return Err("duplicate artifact field");
        }
    }
    const REQUIRED: [&str; 15] = [
        "version",
        "profile",
        "rip",
        "source",
        "seed",
        "index",
        "gpr",
        "flags",
        "address",
        "ram",
        "ram_layout",
        "ram_digest",
        "interpreter_observation",
        "jit_observation",
        "mismatch",
    ];
    if fields.len() != REQUIRED.len() || REQUIRED.iter().any(|key| !fields.contains_key(key)) {
        return Err("artifact fields do not match the single-profile schema");
    }
    if fields["version"].parse::<u32>().map_err(|_| "invalid artifact version")?
        != REPLAY_VERSION
    {
        return Err("unsupported artifact version");
    }
    if fields["profile"] != REPLAY_PROFILE {
        return Err("unsupported replay profile");
    }
    let rip = u64::from_str_radix(fields["rip"], 16).map_err(|_| "invalid RIP")?;
    if rip != ENTRY_RIP {
        return Err("source RIP does not match replay profile");
    }
    let saved_source = unhex(fields["source"])?;
    if saved_source.as_slice() != SOURCE.as_slice() {
        return Err("instruction bytes do not match replay profile");
    }
    let seed = u64::from_str_radix(fields["seed"], 16).map_err(|_| "invalid seed")?;
    if !S3_34DM_SEEDS.contains(&seed) {
        return Err("seed is outside the recorded corpus");
    }
    let index = fields["index"].parse::<usize>().map_err(|_| "invalid index")?;
    if index >= S3_34DM_CASES_PER_SEED {
        return Err("index is outside the recorded corpus");
    }
    let registers = fields["gpr"]
        .split(',')
        .map(|value| u64::from_str_radix(value, 16).map_err(|_| "invalid GPR value"))
        .collect::<Result<Vec<_>, _>>()?;
    let gpr: [u64; 16] = registers
        .try_into()
        .map_err(|_| "artifact must contain all sixteen GPR values")?;
    let flags = u64::from_str_radix(fields["flags"], 16).map_err(|_| "invalid flags")?;
    let address = u32::from_str_radix(fields["address"], 16).map_err(|_| "invalid address")?;
    let ram: [u8; 12] = unhex(fields["ram"])?
        .try_into()
        .map_err(|_| "RAM bytes do not match the fixed profile size")?;
    let input = DwordStoreCorpusInput {
        gpr,
        flags,
        address,
        ram,
    };
    if flags & !DEFINED_FLAGS_MASK != 0 {
        return Err("initial flags exceed the profile's defined-state mask");
    }
    if input.gpr[4] as u32 != address {
        return Err("low32(RSP) does not equal the effective address");
    }
    if !address_matches_profile(index, address) {
        return Err("address is outside the generated class for its provenance index");
    }
    if fields["ram_layout"] != REPLAY_RAM_LAYOUT {
        return Err("RAM layout does not match the fixed profile");
    }
    let actual_layout = ram_layout(&input);
    let mut expected_addresses = Vec::new();
    for offset in -4_i64..=7 {
        if let Some(at) = u64::from(address).checked_add_signed(offset) {
            expected_addresses.push(at);
        }
    }
    if actual_layout.keys().copied().collect::<Vec<_>>() != expected_addresses {
        return Err("RAM address map does not match the fixed profile layout");
    }
    let saved_digest = u64::from_str_radix(fields["ram_digest"], 16)
        .map_err(|_| "invalid RAM digest")?;
    if saved_digest != ram_digest(&input) {
        return Err("RAM digest mismatch");
    }
    let interpreter_observation = String::from_utf8(unhex(fields["interpreter_observation"])?)
        .map_err(|_| "interpreter observation is not UTF-8")?;
    let jit_observation = String::from_utf8(unhex(fields["jit_observation"])?)
        .map_err(|_| "JIT observation is not UTF-8")?;
    let mismatch = String::from_utf8(unhex(fields["mismatch"])?)
        .map_err(|_| "comparator mismatch is not UTF-8")?;
    if mismatch.is_empty() {
        return Err("artifact does not record a comparator mismatch");
    }
    Ok(ReplayArtifact {
        seed,
        index,
        input,
        interpreter_observation,
        jit_observation,
        mismatch,
    })
}

fn write_mismatch_artifact(path: &Path, artifact: &ReplayArtifact) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    file.write_all(serialize_replay_artifact(artifact).as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn save_mismatch_artifact(artifact: &ReplayArtifact) -> std::io::Result<PathBuf> {
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let run_dir = Path::new("work/stage-3/s3-34dn")
        .join(format!("run-{}-{run_id}", std::process::id()));
    fs::create_dir_all(run_dir.parent().expect("run directory has a parent"))?;
    fs::create_dir(&run_dir)?;
    let path = run_dir.join("mismatch.s3-34dn");
    write_mismatch_artifact(&path, artifact)?;
    Ok(path)
}

fn save_reduced_artifact(artifact: &ReplayArtifact) -> std::io::Result<PathBuf> {
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let run_dir = Path::new("work/stage-3/s3-34do")
        .join(format!("run-{}-{run_id}", std::process::id()));
    fs::create_dir_all(run_dir.parent().expect("run directory has a parent"))?;
    fs::create_dir(&run_dir)?;
    let path = run_dir.join("reduced.s3-34do");
    write_mismatch_artifact(&path, artifact)?;
    Ok(path)
}

fn initial_state(input: &DwordStoreCorpusInput) -> LongModeState {
    LongModeState {
        gpr: input.gpr,
        rip: ENTRY_RIP,
        rflags: input.flags,
        ..Default::default()
    }
}

fn run_interpreter(input: &DwordStoreCorpusInput) -> Observation {
    let mut state = initial_state(input);
    let mut ram = TraceRam::from_input(input);
    let outcome = od_isa::execute::step(&mut state, &mut ram, AccessContext::default());
    let (exit, retired_instructions) = match outcome {
        Ok(od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched) => {
            ("Complete".to_owned(), 1)
        }
        Ok(other) => (format!("{other:?}"), 0),
        Err(error) => (format!("Error({error:?})"), 0),
    };
    Observation {
        exit,
        state,
        ram: ram.bytes,
        retired_instructions,
        callbacks: ram.writes,
    }
}

fn run_jit(published: &PublishedCode, input: &DwordStoreCorpusInput) -> Observation {
    let mut state = initial_state(input);
    let mut ram = TraceRam::from_input(input);
    let stop = AtomicU8::new(0);
    // SAFETY: the fixture owns private synchronous RAM; its write callback
    // validates the complete range before committing any byte.
    let result = unsafe {
        published.execute_ir_memory_dword_store_v2(
            &mut state,
            &mut ram,
            AccessContext::default(),
            1,
            None,
            &stop,
        )
    };
    match result {
        Ok(result) => Observation {
            exit: format!("{:?}", result.exit),
            state,
            ram: ram.bytes,
            retired_instructions: result.retired_instructions,
            callbacks: ram.writes,
        },
        Err(error) => Observation {
            exit: format!("Error({error:?})"),
            state,
            ram: ram.bytes,
            retired_instructions: 0,
            callbacks: ram.writes,
        },
    }
}

fn expected(input: &DwordStoreCorpusInput) -> Observation {
    let mut state = initial_state(input);
    state.rip = ENTRY_RIP + SOURCE.len() as u64;
    let mut ram = TraceRam::from_input(input);
    let bytes = state.gpr[0].to_le_bytes()[..4].to_vec();
    for (offset, byte) in bytes.iter().copied().enumerate() {
        ram.bytes.insert(u64::from(input.address) + offset as u64, byte);
    }
    ram.writes.push(WriteCallback {
        address: u64::from(input.address),
        bytes,
    });
    Observation {
        exit: "Complete".into(),
        state,
        ram: ram.bytes,
        retired_instructions: 1,
        callbacks: ram.writes,
    }
}

fn compare(expected: &Observation, actual: &Observation) -> Result<(), &'static str> {
    if expected.exit != actual.exit {
        return Err("exit");
    }
    if expected.state != actual.state {
        return Err("full CPU state");
    }
    if expected.ram != actual.ram {
        return Err("full RAM");
    }
    if expected.retired_instructions != actual.retired_instructions {
        return Err("retirement count");
    }
    if actual.callbacks.len() != 1 || actual.callbacks[0].bytes.len() != 4 {
        return Err("single contiguous width-four callback");
    }
    if expected.callbacks != actual.callbacks {
        return Err("callback trace");
    }
    Ok(())
}

fn comparator_mismatch(
    expected: &Observation,
    interpreter: &Observation,
    jit: &Observation,
) -> Option<String> {
    compare(expected, interpreter)
        .err()
        .map(|field| format!("expected/interpreter: {field}"))
        .or_else(|| {
            compare(expected, jit)
                .err()
                .map(|field| format!("expected/JIT: {field}"))
        })
        .or_else(|| {
            compare(interpreter, jit)
                .err()
                .map(|field| format!("interpreter/JIT: {field}"))
        })
}

fn classify_replay(
    artifact: &ReplayArtifact,
    interpreter_first: &Observation,
    interpreter_second: &Observation,
    jit_first: &Observation,
    jit_second: &Observation,
    mismatch: Option<&str>,
) -> ReplayOutcome {
    if interpreter_first != interpreter_second
        || jit_first != jit_second
        || mismatch != Some(artifact.mismatch.as_str())
        || format!("{interpreter_first:#?}") != artifact.interpreter_observation
        || format!("{jit_first:#?}") != artifact.jit_observation
    {
        ReplayOutcome::NotReproduced
    } else {
        ReplayOutcome::Reproduced
    }
}

fn run_replay_observations(input: &DwordStoreCorpusInput) -> Result<ReplayRuns, String> {
    let expected = expected(input);
    let source_view = CodeView::new(ENTRY_RIP, &SOURCE)
        .map_err(|error| format!("invalid source view: {error:?}"))?;
    let compiled = compile_ir_memory_dword_store_block_v2(source_view, ENTRY_RIP)
    .map_err(|error| format!("compile failed: {error:?}"))?;
    let published = PublishedCode::publish_ir_memory_dword_store_v2(&compiled)
        .map_err(|error| format!("publication failed: {error:?}"))?;
    let interpreter_first = run_interpreter(input);
    let jit_first = run_jit(&published, input);
    let interpreter_second = run_interpreter(input);
    let jit_second = run_jit(&published, input);
    let mismatch_first = comparator_mismatch(&expected, &interpreter_first, &jit_first);
    let mismatch_second = comparator_mismatch(&expected, &interpreter_second, &jit_second);
    Ok(ReplayRuns {
        interpreter_first,
        interpreter_second,
        jit_first,
        jit_second,
        mismatch_first,
        mismatch_second,
    })
}

fn replay_outcome(artifact: &ReplayArtifact, runs: &ReplayRuns) -> ReplayOutcome {
    classify_replay(
        artifact,
        &runs.interpreter_first,
        &runs.interpreter_second,
        &runs.jit_first,
        &runs.jit_second,
        runs.mismatch_first.as_deref(),
    )
}

fn replay_saved_artifact(path: &Path) -> Result<ReplayOutcome, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    // Parsing validates the complete profile, input domain, and RAM layout/digest
    // before compiling or publishing executable code.
    let artifact = parse_replay_artifact(&contents).map_err(str::to_owned)?;
    let runs = run_replay_observations(&artifact.input)?;
    Ok(replay_outcome(&artifact, &runs))
}

fn mapped_ram_indices(input: &DwordStoreCorpusInput) -> Vec<usize> {
    (-4_i64..=7)
        .filter_map(|offset| {
            u64::from(input.address)
                .checked_add_signed(offset)
                .map(|_| (offset + 4) as usize)
        })
        .collect()
}

fn ram_index_for_address(input: &DwordStoreCorpusInput, address: u64) -> Option<usize> {
    (-4_i64..=7).find_map(|offset| {
        u64::from(input.address)
            .checked_add_signed(offset)
            .filter(|mapped| *mapped == address)
            .map(|_| (offset + 4) as usize)
    })
}

fn validate_observation_protocol(
    input: &DwordStoreCorpusInput,
    observation: &Observation,
) -> Result<(), &'static str> {
    if observation.exit != "Complete" || observation.retired_instructions != 1 {
        return Err("backend-error-or-nonterminal-exit");
    }
    if observation.callbacks.len() != 1
        || observation.callbacks[0].address != u64::from(input.address)
        || observation.callbacks[0].bytes.len() != 4
    {
        return Err("unsupported-callback-protocol");
    }
    if observation.ram.keys().copied().collect::<Vec<_>>()
        != ram_layout(input).keys().copied().collect::<Vec<_>>()
    {
        return Err("unsupported-ram-layout");
    }
    for (offset, byte) in observation.callbacks[0].bytes.iter().copied().enumerate() {
        let address = u64::from(input.address) + offset as u64;
        if observation.ram.get(&address) != Some(&byte) {
            return Err("callback-does-not-match-guest-ram");
        }
    }
    Ok(())
}

fn difference_for_pair(
    input: &DwordStoreCorpusInput,
    pair: BackendPair,
    left: &Observation,
    right: &Observation,
) -> Result<Option<PairMismatch>, &'static str> {
    validate_observation_protocol(input, left)?;
    validate_observation_protocol(input, right)?;
    if left.state.gpr[4] != input.gpr[4] || right.state.gpr[4] != input.gpr[4] {
        return Err("unsupported-address-register-mismatch");
    }
    let mut left_aux = left.state.clone();
    let mut right_aux = right.state.clone();
    left_aux.gpr = [0; 16];
    right_aux.gpr = [0; 16];
    if left_aux != right_aux {
        return Err("unsupported-non-gpr-cpu-mismatch");
    }

    let gpr_differences = (0..16)
        .filter(|index| *index != 4 && left.state.gpr[*index] != right.state.gpr[*index])
        .map(MismatchField::Gpr)
        .collect::<Vec<_>>();
    let mut ram_differences = Vec::new();
    for (address, left_byte) in &left.ram {
        let right_byte = right.ram.get(address).ok_or("unsupported-ram-layout")?;
        if left_byte != right_byte {
            ram_differences.push(MismatchField::Ram(
                ram_index_for_address(input, *address).ok_or("unsupported-ram-address")?,
            ));
        }
    }
    if !gpr_differences.is_empty() && !ram_differences.is_empty() {
        return Err("multi-class-gpr-and-ram-mismatch");
    }
    let fields = if !gpr_differences.is_empty() {
        gpr_differences
    } else {
        ram_differences
    };
    if fields.is_empty() {
        return Ok(None);
    }
    Ok(Some(PairMismatch {
        pair,
        fields,
    }))
}

fn signature_for_observations(
    input: &DwordStoreCorpusInput,
    expected: &Observation,
    interpreter: &Observation,
    jit: &Observation,
) -> Result<MismatchSignature, &'static str> {
    let mut pairs = Vec::new();
    for (pair, left, right) in [
        (
            BackendPair::ExpectedInterpreter,
            expected,
            interpreter,
        ),
        (BackendPair::ExpectedJit, expected, jit),
        (BackendPair::InterpreterJit, interpreter, jit),
    ] {
        if let Some(difference) = difference_for_pair(input, pair, left, right)? {
            pairs.push(difference);
        }
    }
    if pairs.is_empty() {
        return Err("no-supported-value-mismatch");
    }
    Ok(MismatchSignature { pairs })
}

fn stable_mismatch_signature(
    input: &DwordStoreCorpusInput,
    runs: &ReplayRuns,
) -> Result<MismatchSignature, &'static str> {
    if runs.interpreter_first != runs.interpreter_second || runs.jit_first != runs.jit_second {
        return Err("unstable-backend-observations");
    }
    if runs.mismatch_first.is_none() || runs.mismatch_first != runs.mismatch_second {
        return Err("unstable-or-absent-comparator-mismatch");
    }
    let expected = expected(input);
    let first = signature_for_observations(
        input,
        &expected,
        &runs.interpreter_first,
        &runs.jit_first,
    )?;
    let second = signature_for_observations(
        input,
        &expected,
        &runs.interpreter_second,
        &runs.jit_second,
    )?;
    if first != second {
        return Err("unstable-mismatch-signature");
    }
    Ok(first)
}

fn candidate_passes_ordinary_replay(
    artifact: &ReplayArtifact,
    runs: &ReplayRuns,
    required_signature: &MismatchSignature,
) -> bool {
    runs.interpreter_first == runs.interpreter_second
        && runs.jit_first == runs.jit_second
        && runs.mismatch_first.as_deref() == Some(artifact.mismatch.as_str())
        && runs.mismatch_first == runs.mismatch_second
        && format!("{:#?}", runs.interpreter_first) == artifact.interpreter_observation
        && format!("{:#?}", runs.jit_first) == artifact.jit_observation
        && matches!(
            stable_mismatch_signature(&artifact.input, runs),
            Ok(signature) if &signature == required_signature
        )
}

fn mutable_value_popcount(input: &DwordStoreCorpusInput) -> u32 {
    let register_bits = input
        .gpr
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 4)
        .map(|(_, value)| value.count_ones())
        .sum::<u32>();
    register_bits
        + mapped_ram_indices(input)
            .into_iter()
            .map(|index| u32::from(input.ram[index].count_ones()))
            .sum::<u32>()
}

fn value_only_mutation_preserves_profile(
    before: &DwordStoreCorpusInput,
    after: &DwordStoreCorpusInput,
) -> bool {
    before.address == after.address
        && before.gpr[4] == after.gpr[4]
        && before.flags == after.flags
        && (0..before.ram.len()).all(|index| {
            mapped_ram_indices(before).contains(&index) || before.ram[index] == after.ram[index]
        })
}

fn push_value_candidate(
    candidates: &mut Vec<ValueCandidate>,
    input: &DwordStoreCorpusInput,
    mutation: ValueMutation,
    limit: usize,
) {
    if candidates.len() >= limit {
        return;
    }
    let mut candidate = input.clone();
    match mutation {
        ValueMutation::ZeroGpr(index) => candidate.gpr[index] = 0,
        ValueMutation::ClearGprBit { index, bit } => candidate.gpr[index] &= !(1_u64 << bit),
        ValueMutation::ZeroRam(index) => candidate.ram[index] = 0,
        ValueMutation::ClearRamBit { index, bit } => candidate.ram[index] &= !(1_u8 << bit),
    }
    if candidate != *input && mutable_value_popcount(&candidate) < mutable_value_popcount(input) {
        candidates.push(ValueCandidate {
            mutation,
            input: candidate,
        });
    }
}

fn value_candidates(input: &DwordStoreCorpusInput, limit: usize) -> Vec<ValueCandidate> {
    let mut candidates = Vec::new();
    for index in 0..16 {
        if index != 4 && input.gpr[index] != 0 {
            push_value_candidate(
                &mut candidates,
                input,
                ValueMutation::ZeroGpr(index),
                limit,
            );
        }
        if candidates.len() >= limit {
            return candidates;
        }
    }
    let mapped_ram = mapped_ram_indices(input);
    for index in &mapped_ram {
        if input.ram[*index] != 0 {
            push_value_candidate(
                &mut candidates,
                input,
                ValueMutation::ZeroRam(*index),
                limit,
            );
        }
        if candidates.len() >= limit {
            return candidates;
        }
    }
    for index in 0..16 {
        if index == 4 {
            continue;
        }
        for bit in 0..64 {
            if input.gpr[index] & (1_u64 << bit) != 0 {
                push_value_candidate(
                    &mut candidates,
                    input,
                    ValueMutation::ClearGprBit { index, bit },
                    limit,
                );
            }
            if candidates.len() >= limit {
                return candidates;
            }
        }
    }
    for index in mapped_ram {
        for bit in 0..8 {
            if input.ram[index] & (1_u8 << bit) != 0 {
                push_value_candidate(
                    &mut candidates,
                    input,
                    ValueMutation::ClearRamBit { index, bit },
                    limit,
                );
            }
            if candidates.len() >= limit {
                return candidates;
            }
        }
    }
    candidates
}

fn reduce_saved_artifact(path: &Path) -> Result<ReductionReport, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    // The strict single-profile parser runs before any publication or execution.
    let original = parse_replay_artifact(&contents).map_err(str::to_owned)?;
    let original_runs = match run_replay_observations(&original.input) {
        Ok(runs) => runs,
        Err(_) => {
            return Ok(ReductionReport {
                outcome: "rejected",
                attempts: 0,
                observations: 0,
                stop_reason: "original_backend_error".into(),
                reduced_artifact: None,
            });
        }
    };
    let mut observations = 4;
    if replay_outcome(&original, &original_runs) != ReplayOutcome::Reproduced {
        return Ok(ReductionReport {
            outcome: "not_reproduced",
            attempts: 0,
            observations,
            stop_reason: "original_mismatch_not_reproduced_twice".into(),
            reduced_artifact: None,
        });
    }
    let original_signature = match stable_mismatch_signature(&original.input, &original_runs) {
        Ok(signature) => signature,
        Err(reason) => {
            return Ok(ReductionReport {
                outcome: "rejected",
                attempts: 0,
                observations,
                stop_reason: reason.into(),
                reduced_artifact: None,
            });
        }
    };
    let original_popcount = mutable_value_popcount(&original.input);
    let mut best_artifact = original.clone();
    let mut best_runs = original_runs;
    let mut best_popcount = original_popcount;
    let mut attempts = 0;
    let mut stop_reason = String::from("no_signature_preserving_candidate");

    'reduce: loop {
        if attempts >= MAX_REDUCER_CANDIDATES {
            stop_reason = "candidate_cap_reached".into();
            break;
        }
        let candidates = value_candidates(
            &best_artifact.input,
            MAX_REDUCER_CANDIDATES - attempts,
        );
        if candidates.is_empty() {
            stop_reason = "no_lower_popcount_candidate".into();
            break;
        }
        let mut accepted = false;
        for candidate in candidates {
            if attempts >= MAX_REDUCER_CANDIDATES {
                stop_reason = "candidate_cap_reached".into();
                break 'reduce;
            }
            attempts += 1;
            if !value_only_mutation_preserves_profile(&best_artifact.input, &candidate.input)
                || mutable_value_popcount(&candidate.input) >= best_popcount
            {
                stop_reason = "candidate_changed_fixed_profile_or_did_not_reduce_values".into();
                break 'reduce;
            }
            let candidate_runs = match run_replay_observations(&candidate.input) {
                Ok(runs) => runs,
                Err(_) => {
                    stop_reason = "candidate_backend_error".into();
                    break 'reduce;
                }
            };
            observations += 4;
            if candidate_runs.mismatch_first.as_deref() != Some(original.mismatch.as_str())
                || !matches!(
                    stable_mismatch_signature(&candidate.input, &candidate_runs),
                    Ok(signature) if &signature == &original_signature
                )
            {
                continue;
            }
            let candidate_mismatch = candidate_runs
                .mismatch_first
                .clone()
                .expect("stable mismatch signature requires a mismatch");
            let candidate_artifact = ReplayArtifact {
                seed: original.seed,
                index: original.index,
                input: candidate.input,
                interpreter_observation: format!("{:#?}", candidate_runs.interpreter_first),
                jit_observation: format!("{:#?}", candidate_runs.jit_first),
                mismatch: candidate_mismatch,
            };
            // Re-run through the ordinary artifact replay contract before this
            // candidate can become the next best input or be persisted.
            let verification_runs = match run_replay_observations(&candidate_artifact.input) {
                Ok(runs) => runs,
                Err(_) => {
                    stop_reason = "candidate_verification_backend_error".into();
                    break 'reduce;
                }
            };
            observations += 4;
            if !candidate_passes_ordinary_replay(
                &candidate_artifact,
                &verification_runs,
                &original_signature,
            ) {
                continue;
            }
            best_popcount = mutable_value_popcount(&candidate_artifact.input);
            best_artifact = candidate_artifact;
            best_runs = verification_runs;
            accepted = true;
            break;
        }
        if accepted {
            continue;
        }
        if attempts >= MAX_REDUCER_CANDIDATES {
            stop_reason = "candidate_cap_reached".into();
        }
        break;
    }

    let reduced_artifact = if best_popcount < original_popcount {
        // Keep this final guard adjacent to publication: persistence is allowed
        // only after the ordinary repeat-and-observation replay just passed.
        if !candidate_passes_ordinary_replay(
            &best_artifact,
            &best_runs,
            &original_signature,
        ) {
            return Ok(ReductionReport {
                outcome: "rejected",
                attempts,
                observations,
                stop_reason: "final_candidate_failed_ordinary_replay".into(),
                reduced_artifact: None,
            });
        }
        Some(save_reduced_artifact(&best_artifact).map_err(|error| error.to_string())?)
    } else {
        None
    };
    Ok(ReductionReport {
        outcome: if reduced_artifact.is_some() {
            "reduced"
        } else {
            "reproduced_no_reduction"
        },
        attempts,
        observations,
        stop_reason,
        reduced_artifact,
    })
}

fn synthetic_replay_runs(
    input: &DwordStoreCorpusInput,
    interpreter: &Observation,
    jit: &Observation,
) -> ReplayRuns {
    let expected = expected(input);
    let mismatch = comparator_mismatch(&expected, interpreter, jit);
    ReplayRuns {
        interpreter_first: interpreter.clone(),
        interpreter_second: interpreter.clone(),
        jit_first: jit.clone(),
        jit_second: jit.clone(),
        mismatch_first: mismatch.clone(),
        mismatch_second: mismatch,
    }
}

fn assert_comparator_rejects_corruption(baseline: &Observation) {
    let mut corrupted = baseline.clone();
    corrupted.state.gpr[0] ^= 1;
    assert_eq!(compare(baseline, &corrupted), Err("full CPU state"));

    let mut corrupted = baseline.clone();
    corrupted.state.rip ^= 1;
    assert_eq!(compare(baseline, &corrupted), Err("full CPU state"));

    let mut corrupted = baseline.clone();
    let first_ram_address = *corrupted.ram.keys().next().expect("fixture RAM is nonempty");
    *corrupted.ram.get_mut(&first_ram_address).unwrap() ^= 1;
    assert_eq!(compare(baseline, &corrupted), Err("full RAM"));

    let mut corrupted = baseline.clone();
    corrupted.retired_instructions += 1;
    assert_eq!(compare(baseline, &corrupted), Err("retirement count"));

    let mut corrupted = baseline.clone();
    corrupted.callbacks[0].bytes[0] ^= 1;
    assert_eq!(compare(baseline, &corrupted), Err("callback trace"));
}

#[test]
fn fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input() {
    let compiled = compile_ir_memory_dword_store_block_v2(
        CodeView::new(ENTRY_RIP, &SOURCE).unwrap(),
        ENTRY_RIP,
    )
    .unwrap();
    let published = PublishedCode::publish_ir_memory_dword_store_v2(&compiled).unwrap();

    let first_input = generate_s3_34dm_dword_store_input(S3_34DM_SEEDS[0], 0);
    let first_expected = expected(&first_input);
    assert_comparator_rejects_corruption(&first_expected);

    let mut unique = HashSet::new();
    for seed in S3_34DM_SEEDS {
        for index in 0..S3_34DM_CASES_PER_SEED {
            let input = generate_s3_34dm_dword_store_input(seed, index);
            assert!(unique.insert(input.clone()), "duplicate input seed={seed:#x} index={index}");
            match index % 4 {
                0 => assert_eq!(input.address, 0),
                1 => assert_ne!(input.address % 4, 0, "unaligned class seed={seed:#x} index={index}"),
                2 => assert!((0x1000..0x8000_1000).contains(&input.address)),
                3 => assert!(u64::from(input.address) + 4 > (1_u64 << 32)),
                _ => unreachable!(),
            }
            let expected = expected(&input);
            for repetition in 0..2 {
                let interpreter = run_interpreter(&input);
                let native = run_jit(&published, &input);
                if let Some(mismatch) = comparator_mismatch(&expected, &interpreter, &native) {
                    let artifact = ReplayArtifact {
                        seed,
                        index,
                        input: input.clone(),
                        interpreter_observation: format!("{interpreter:#?}"),
                        jit_observation: format!("{native:#?}"),
                        mismatch: mismatch.clone(),
                    };
                    let artifact_path = save_mismatch_artifact(&artifact)
                        .unwrap_or_else(|error| panic!("could not persist mismatch artifact: {error}"));
                    panic!(
                        "S3-34dm mismatch: seed={seed:#018x} index={index} repetition={repetition} comparator={mismatch}\nartifact={}\ninput={input:#?}\nexpected={expected:#?}\ninterpreter={interpreter:#?}\nJIT={native:#?}",
                        artifact_path.display(),
                    );
                }
            }
        }
    }
    assert_eq!(unique.len(), S3_34DM_SEEDS.len() * S3_34DM_CASES_PER_SEED);
}

#[test]
fn s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed() {
    let input = generate_s3_34dm_dword_store_input(S3_34DM_SEEDS[0], 0);
    let baseline = expected(&input);

    // This is deliberately synthetic source-level evidence; it does not execute
    // either backend and cannot establish a guest or JIT defect.
    let mut synthetic_interpreter = baseline.clone();
    synthetic_interpreter.state.gpr[0] ^= 1;
    let synthetic_jit = baseline.clone();
    let synthetic_mismatch = comparator_mismatch(
        &baseline,
        &synthetic_interpreter,
        &synthetic_jit,
    )
    .expect("synthetic observations must differ");
    let artifact = ReplayArtifact {
        seed: S3_34DM_SEEDS[0],
        index: 0,
        input,
        interpreter_observation: format!("{synthetic_interpreter:#?}"),
        jit_observation: format!("{synthetic_jit:#?}"),
        mismatch: synthetic_mismatch,
    };
    let serialized = serialize_replay_artifact(&artifact);
    assert_eq!(parse_replay_artifact(&serialized), Ok(artifact.clone()));

    let mut invalid_profile = serialized.clone();
    invalid_profile = invalid_profile.replace(REPLAY_PROFILE, "other-profile");
    assert_eq!(
        parse_replay_artifact(&invalid_profile),
        Err("unsupported replay profile")
    );

    let recompute_ram_digest = |contents: String| {
        let digest_field = format!("ram_digest={:016x}", ram_digest(&artifact.input));
        contents
            .lines()
            .map(|line| {
                if line.starts_with("ram_digest=") {
                    digest_field.as_str()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    let invalid_version = recompute_ram_digest(serialized.replace("version=1\n", "version=2\n"));
    assert_eq!(
        parse_replay_artifact(&invalid_version),
        Err("unsupported artifact version")
    );

    let invalid_rip = recompute_ram_digest(serialized.replace(
        &format!("rip={ENTRY_RIP:016x}"),
        &format!("rip={:016x}", ENTRY_RIP + 1),
    ));
    assert_eq!(
        parse_replay_artifact(&invalid_rip),
        Err("source RIP does not match replay profile")
    );

    let invalid_source = serialized.replace("source=67890424", "source=67890425");
    assert_eq!(
        parse_replay_artifact(&invalid_source),
        Err("instruction bytes do not match replay profile")
    );

    let mut invalid_address_artifact = artifact.clone();
    invalid_address_artifact.input.address = 2;
    invalid_address_artifact.input.gpr[4] =
        (invalid_address_artifact.input.gpr[4] & 0xffff_ffff_0000_0000) | 2;
    let invalid_address = serialize_replay_artifact(&invalid_address_artifact);
    assert_eq!(
        parse_replay_artifact(&invalid_address),
        Err("address is outside the generated class for its provenance index")
    );

    let bad_digest = ram_digest(&artifact.input) ^ 1;
    let invalid_digest = serialized.replace(
        &format!("ram_digest={:016x}", ram_digest(&artifact.input)),
        &format!("ram_digest={bad_digest:016x}"),
    );
    assert_eq!(parse_replay_artifact(&invalid_digest), Err("RAM digest mismatch"));

    let invalid_layout = serialized.replace(REPLAY_RAM_LAYOUT, "offsets:0..=11");
    assert_eq!(
        parse_replay_artifact(&invalid_layout),
        Err("RAM layout does not match the fixed profile")
    );

    let temporary_path = std::env::temp_dir().join(format!(
        "s3-34dn-self-check-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    ));
    fs::create_dir(&temporary_path).expect("create isolated self-check directory");
    let _cleanup = TemporaryArtifactDirectory(temporary_path.clone());
    let sentinel_path = temporary_path.join("existing.s3-34dn");
    let sentinel = b"preserve this existing artifact";
    fs::write(&sentinel_path, sentinel).expect("precreate sentinel artifact");
    let write_error = write_mismatch_artifact(&sentinel_path, &artifact)
        .expect_err("create-only writer must reject an existing file");
    let sentinel_after = fs::read(&sentinel_path).expect("read preserved sentinel artifact");
    assert_eq!(write_error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(sentinel_after, sentinel);

    assert_eq!(
        classify_replay(
            &artifact,
            &synthetic_interpreter,
            &synthetic_interpreter,
            &synthetic_jit,
            &synthetic_jit,
            Some(&artifact.mismatch),
        ),
        ReplayOutcome::Reproduced,
        "the synthetic comparator mismatch is only a classifier self-check"
    );
    assert_eq!(
        classify_replay(
            &artifact,
            &baseline,
            &baseline,
            &baseline,
            &baseline,
            None,
        ),
        ReplayOutcome::NotReproduced,
        "absence of a matching prior mismatch is not_reproduced"
    );
}

#[test]
fn replay_saved_s3_34dn_mismatch_artifact() {
    let Some(path) = std::env::var_os(REPLAY_ENV).map(PathBuf::from) else {
        eprintln!("S3-34dn replay: not_reproduced (no prior mismatch artifact supplied)");
        return;
    };
    if !path.is_file() {
        eprintln!("S3-34dn replay: not_reproduced (prior mismatch artifact is absent)");
        return;
    }
    match replay_saved_artifact(&path) {
        Ok(ReplayOutcome::NotReproduced) => {
            eprintln!("S3-34dn replay: not_reproduced (the deterministic mismatch did not recur)");
        }
        Ok(ReplayOutcome::Reproduced) => {
            eprintln!(
                "S3-34dn replay: reproduced in both reset-state repetitions; this is replay evidence, not defect confirmation"
            );
        }
        Err(error) => panic!("invalid S3-34dn replay artifact: {error}"),
    }
}

#[test]
fn reduce_saved_s3_34do_value_only_artifact() {
    let Some(path) = std::env::var_os(REDUCE_ENV).map(PathBuf::from) else {
        eprintln!(
            "S3-34do reducer: outcome=not_reproduced attempts=0 observations=0 stop_reason=no_artifact"
        );
        return;
    };
    if !path.is_file() {
        eprintln!(
            "S3-34do reducer: outcome=not_reproduced attempts=0 observations=0 stop_reason=artifact_absent"
        );
        return;
    }
    match reduce_saved_artifact(&path) {
        Ok(report) => eprintln!(
            "S3-34do reducer: outcome={} attempts={} observations={} stop_reason={} reduced_artifact={}",
            report.outcome,
            report.attempts,
            report.observations,
            report.stop_reason,
            report
                .reduced_artifact
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".into()),
        ),
        Err(error) => panic!("invalid S3-34do reducer input: {error}"),
    }
}

#[test]
fn s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature() {
    // All observations below are synthetic values. This self-check does not run
    // either backend or invoke the artifact reducer on a real mismatch.
    let mut input = generate_s3_34dm_dword_store_input(S3_34DM_SEEDS[0], 0);
    input.gpr = [0; 16];
    input.gpr[4] = (0x1234_5678_u64 << 32) | u64::from(input.address);
    input.gpr[0] = 0x0f;
    input.ram = [0xff, 0xaa, 0xbb, 0xcc, 0x03, 0, 0, 0, 0, 0, 0, 0];
    let baseline = expected(&input);
    let mut synthetic_interpreter = baseline.clone();
    synthetic_interpreter.state.gpr[1] ^= 1;
    let synthetic_jit = baseline.clone();
    let original_runs = synthetic_replay_runs(&input, &synthetic_interpreter, &synthetic_jit);
    let original_signature = stable_mismatch_signature(&input, &original_runs)
        .expect("synthetic one-GPR mismatch is supported");
    assert_eq!(
        original_signature,
        MismatchSignature {
            pairs: vec![
                PairMismatch {
                    pair: BackendPair::ExpectedInterpreter,
                    fields: vec![MismatchField::Gpr(1)],
                },
                PairMismatch {
                    pair: BackendPair::InterpreterJit,
                    fields: vec![MismatchField::Gpr(1)],
                },
            ],
        }
    );

    let candidates = value_candidates(&input, MAX_REDUCER_CANDIDATES);
    assert!(candidates.len() <= MAX_REDUCER_CANDIDATES);
    assert_eq!(candidates, value_candidates(&input, MAX_REDUCER_CANDIDATES));
    assert_eq!(candidates[0].mutation, ValueMutation::ZeroGpr(0));
    assert!(candidates
        .iter()
        .any(|candidate| matches!(candidate.mutation, ValueMutation::ZeroRam(index) if index == 4)));
    assert!(candidates
        .iter()
        .any(|candidate| matches!(candidate.mutation, ValueMutation::ClearGprBit { index: 0, .. })));
    assert!(candidates
        .iter()
        .any(|candidate| matches!(candidate.mutation, ValueMutation::ClearRamBit { index: 4, .. })));
    for candidate in &candidates {
        assert!(value_only_mutation_preserves_profile(&input, &candidate.input));
        assert_eq!(candidate.input.address, input.address);
        assert_eq!(candidate.input.gpr[4], input.gpr[4]);
        assert_eq!(candidate.input.flags, input.flags);
        assert_eq!(&candidate.input.ram[..4], &input.ram[..4]);
        assert!(mutable_value_popcount(&candidate.input) < mutable_value_popcount(&input));
        match candidate.mutation {
            ValueMutation::ZeroGpr(index) | ValueMutation::ClearGprBit { index, .. } => {
                assert_ne!(index, 4, "RSP is immutable in the reducer");
            }
            ValueMutation::ZeroRam(index) | ValueMutation::ClearRamBit { index, .. } => {
                assert!(mapped_ram_indices(&input).contains(&index));
                assert!(index >= 4, "address-zero underflow bytes are unmapped");
            }
        }
    }

    let first = &candidates[0];
    let candidate_expected = expected(&first.input);
    let mut candidate_interpreter = candidate_expected.clone();
    candidate_interpreter.state.gpr[1] ^= 1;
    let candidate_jit = candidate_expected.clone();
    let candidate_runs = synthetic_replay_runs(
        &first.input,
        &candidate_interpreter,
        &candidate_jit,
    );
    assert_eq!(
        stable_mismatch_signature(&first.input, &candidate_runs),
        Ok(original_signature.clone()),
        "the synthetic candidate preserves the exact pair and GPR index"
    );
    let candidate_artifact = ReplayArtifact {
        seed: S3_34DM_SEEDS[0],
        index: 0,
        input: first.input.clone(),
        interpreter_observation: format!("{candidate_interpreter:#?}"),
        jit_observation: format!("{candidate_jit:#?}"),
        mismatch: candidate_runs.mismatch_first.clone().unwrap(),
    };
    assert!(candidate_passes_ordinary_replay(
        &candidate_artifact,
        &candidate_runs,
        &original_signature,
    ));

    let mut synthetic_ram_interpreter = baseline.clone();
    let first_store_address = u64::from(input.address);
    synthetic_ram_interpreter.ram.insert(
        first_store_address,
        synthetic_ram_interpreter.ram[&first_store_address] ^ 1,
    );
    synthetic_ram_interpreter.callbacks[0].bytes[0] ^= 1;
    let ram_signature = signature_for_observations(
        &input,
        &baseline,
        &synthetic_ram_interpreter,
        &baseline,
    )
    .expect("synthetic RAM byte mismatch has a mapped signature");
    assert_eq!(
        ram_signature.pairs,
        vec![
            PairMismatch {
                pair: BackendPair::ExpectedInterpreter,
                fields: vec![MismatchField::Ram(4)],
            },
            PairMismatch {
                pair: BackendPair::InterpreterJit,
                fields: vec![MismatchField::Ram(4)],
            },
        ],
        "address-zero store maps to the retained RAM slice, not discarded underflow bytes"
    );

    let mut synthetic_multi_class = synthetic_ram_interpreter.clone();
    synthetic_multi_class.state.gpr[1] ^= 1;
    assert_eq!(
        signature_for_observations(&input, &baseline, &synthetic_multi_class, &baseline),
        Err("multi-class-gpr-and-ram-mismatch")
    );

    let mut shared_interpreter = baseline.clone();
    shared_interpreter.state.gpr[0] ^= 1;
    let mut shared_jit = baseline.clone();
    shared_jit.state.gpr[0] ^= 1;
    shared_jit.state.gpr[1] ^= 1;
    let shared_runs = synthetic_replay_runs(&input, &shared_interpreter, &shared_jit);
    let shared_signature = stable_mismatch_signature(&input, &shared_runs)
        .expect("synthetic shared and pairwise GPR mismatch is supported");
    assert_eq!(
        shared_signature.pairs,
        vec![
            PairMismatch {
                pair: BackendPair::ExpectedInterpreter,
                fields: vec![MismatchField::Gpr(0)],
            },
            PairMismatch {
                pair: BackendPair::ExpectedJit,
                fields: vec![MismatchField::Gpr(0), MismatchField::Gpr(1)],
            },
            PairMismatch {
                pair: BackendPair::InterpreterJit,
                fields: vec![MismatchField::Gpr(1)],
            },
        ]
    );
    let mut common_mismatch_removed_jit = baseline.clone();
    common_mismatch_removed_jit.state.gpr[1] ^= 1;
    let removed_common_runs = synthetic_replay_runs(
        &input,
        &baseline,
        &common_mismatch_removed_jit,
    );
    let removed_common_signature =
        stable_mismatch_signature(&input, &removed_common_runs).unwrap();
    assert_ne!(removed_common_signature, shared_signature);
    assert_eq!(
        removed_common_signature.pairs,
        vec![
            PairMismatch {
                pair: BackendPair::ExpectedJit,
                fields: vec![MismatchField::Gpr(1)],
            },
            PairMismatch {
                pair: BackendPair::InterpreterJit,
                fields: vec![MismatchField::Gpr(1)],
            },
        ],
        "the pairwise GPR mismatch remains after the common expected/interpreter field disappears"
    );
    let removed_common_artifact = ReplayArtifact {
        seed: S3_34DM_SEEDS[0],
        index: 0,
        input: input.clone(),
        interpreter_observation: format!("{baseline:#?}"),
        jit_observation: format!("{common_mismatch_removed_jit:#?}"),
        mismatch: removed_common_runs.mismatch_first.clone().unwrap(),
    };
    assert!(!candidate_passes_ordinary_replay(
        &removed_common_artifact,
        &removed_common_runs,
        &shared_signature,
    ));

    let mut unstable_runs = original_runs;
    unstable_runs.interpreter_second.state.gpr[1] ^= 2;
    assert_eq!(
        stable_mismatch_signature(&input, &unstable_runs),
        Err("unstable-backend-observations")
    );
}
