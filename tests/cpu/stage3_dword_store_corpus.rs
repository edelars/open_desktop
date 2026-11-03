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
const REPLAY_RAM_LAYOUT: &str = "offsets:-4..=7;omit-address-underflow";
const DEFINED_FLAGS_MASK: u64 = 0x8d5;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplayOutcome {
    NotReproduced,
    Reproduced,
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
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    file.write_all(serialize_replay_artifact(artifact).as_bytes())?;
    file.sync_all()?;
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

fn replay_saved_artifact(path: &Path) -> Result<ReplayOutcome, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    // Parsing validates the complete profile, input domain, and RAM layout/digest
    // before compiling or publishing executable code.
    let artifact = parse_replay_artifact(&contents).map_err(str::to_owned)?;
    let source_view = CodeView::new(ENTRY_RIP, &SOURCE)
        .map_err(|error| format!("invalid source view: {error:?}"))?;
    let compiled = compile_ir_memory_dword_store_block_v2(source_view, ENTRY_RIP)
    .map_err(|error| format!("compile failed: {error:?}"))?;
    let published = PublishedCode::publish_ir_memory_dword_store_v2(&compiled)
        .map_err(|error| format!("publication failed: {error:?}"))?;
    let expected = expected(&artifact.input);
    let interpreter_first = run_interpreter(&artifact.input);
    let jit_first = run_jit(&published, &artifact.input);
    let interpreter_second = run_interpreter(&artifact.input);
    let jit_second = run_jit(&published, &artifact.input);
    let mismatch = comparator_mismatch(&expected, &interpreter_first, &jit_first);
    Ok(classify_replay(
        &artifact,
        &interpreter_first,
        &interpreter_second,
        &jit_first,
        &jit_second,
        mismatch.as_deref(),
    ))
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

    let invalid_source = serialized.replace("source=67890424", "source=67890425");
    assert_eq!(
        parse_replay_artifact(&invalid_source),
        Err("instruction bytes do not match replay profile")
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
