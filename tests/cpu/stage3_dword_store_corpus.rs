#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicU8;

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
                for (backend, actual) in [("interpreter", &interpreter), ("JIT", &native)] {
                    if let Err(field) = compare(&expected, actual) {
                        panic!(
                            "S3-34dm mismatch: seed={seed:#018x} index={index} repetition={repetition} backend={backend} field={field}\ninput={input:#?}\nexpected={expected:#?}\nactual={actual:#?}"
                        );
                    }
                }
                if let Err(field) = compare(&interpreter, &native) {
                    panic!(
                        "S3-34dm backend mismatch: seed={seed:#018x} index={index} repetition={repetition} field={field}\ninput={input:#?}\ninterpreter={interpreter:#?}\nJIT={native:#?}"
                    );
                }
            }
        }
    }
    assert_eq!(unique.len(), S3_34DM_SEEDS.len() * S3_34DM_CASES_PER_SEED);
}
