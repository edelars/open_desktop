//! Deterministic fixtures for the implemented register-only CPU profile.

use crate::case::{
    ComparisonScope, CpuProfile, ExceptionComparisonScope, ExpectedExit, FIXTURE_SCHEMA_VERSION,
    Fixture, InitialState, MemoryComparisonScope, Program, Provenance, Register,
};
use crate::{HexU64, sha256_hex};

pub const GENERATOR_VERSION: &str = "od-testkit-cpu-v1";
const ARITHMETIC_FLAGS: u64 = 0x8d5;
const VALUE_BOUNDARIES: [u64; 12] = [
    0,
    1,
    2,
    0x7f,
    0x80,
    0xff,
    0x100,
    0x7fff_ffff_ffff_ffff,
    0x8000_0000_0000_0000,
    u64::MAX - 1,
    u64::MAX,
    0x0101_0101_0101_0101,
];

/// Create one deterministic, acyclic fixture from a 64-bit seed.
///
/// The generated program uses only MOV imm64, ADD/SUB/CMP r64,r64, and a
/// forward JZ/JNZ that can skip one MOV. Seed and sequence identify the exact
/// stream; generator semantics are versioned by [`GENERATOR_VERSION`].
pub fn generate_fixture(seed: u64, sequence: u32) -> Fixture {
    let mut random = SplitMix64(seed ^ (u64::from(sequence).wrapping_mul(0x9e37_79b9_7f4a_7c15)));
    let mut initial = [0; 4];
    for register in &mut initial {
        *register = random.value();
    }

    let destination = random.index(4) as u8;
    let source = random.index(4) as u8;
    let initial_immediate = random.value();
    let final_immediate = random.value();
    let operation = random.index(3);
    let branch = random.index(3) != 0;
    let branch_on_zero = random.index(2) == 0;

    let mut program = Vec::with_capacity(26);
    encode_mov(&mut program, destination, initial_immediate);
    encode_register_operation(&mut program, operation, destination, source);
    if branch {
        program.extend_from_slice(if branch_on_zero {
            &[0x74, 0x0a]
        } else {
            &[0x75, 0x0a]
        });
        encode_mov(&mut program, destination, final_immediate);
    }

    let case_id = format!("gen_v1_{seed:016x}_{sequence:08x}");
    Fixture {
        schema_version: FIXTURE_SCHEMA_VERSION,
        case_id,
        profile: CpuProfile::S0_09RegisterSubsetV1,
        provenance: Provenance {
            kind: "deterministic_generator".to_string(),
            source: format!("{GENERATOR_VERSION}; seed={seed}; sequence={sequence}"),
            specification: "docs/specs/CPU_CONTRACT.md".to_string(),
            specification_version: "v0.1; Intel SDM 093".to_string(),
        },
        seed: Some(seed ^ u64::from(sequence)),
        program: Program {
            sha256: sha256_hex(&program),
            bytes_hex: encode_hex(&program),
        },
        initial_state: InitialState {
            rax: HexU64::new(initial[0]),
            rcx: HexU64::new(initial[1]),
            rdx: HexU64::new(initial[2]),
            rbx: HexU64::new(initial[3]),
            rip: HexU64::new(0),
            rflags: HexU64::new(2),
        },
        instruction_budget: 64,
        wall_timeout_ms: 1_000,
        comparison: ComparisonScope {
            registers: vec![Register::Rax, Register::Rcx, Register::Rdx, Register::Rbx],
            rip: true,
            rflags_mask: HexU64::new(ARITHMETIC_FLAGS),
            memory: MemoryComparisonScope::Unsupported,
            exceptions: ExceptionComparisonScope::Unsupported,
        },
        expected_exit: ExpectedExit::ReachedCodeEnd,
    }
}

fn encode_mov(output: &mut Vec<u8>, register: u8, value: u64) {
    output.extend_from_slice(&[0x48, 0xb8 + register]);
    output.extend_from_slice(&value.to_le_bytes());
}

fn encode_register_operation(output: &mut Vec<u8>, operation: usize, destination: u8, source: u8) {
    let opcode = match operation {
        0 => 0x03, // ADD r64, r/m64
        1 => 0x2b, // SUB r64, r/m64
        _ => 0x3b, // CMP r64, r/m64
    };
    output.extend_from_slice(&[0x48, opcode, 0xc0 | (destination << 3) | source]);
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn index(&mut self, upper: usize) -> usize {
        (self.next() as usize) % upper
    }

    fn value(&mut self) -> u64 {
        let choice = self.index(4);
        if choice == 0 {
            VALUE_BOUNDARIES[self.index(VALUE_BOUNDARIES.len())]
        } else {
            self.next()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GENERATOR_VERSION, encode_register_operation, generate_fixture};
    use crate::case::Fixture;
    use std::collections::HashSet;

    #[test]
    fn seed_and_sequence_reproduce_identical_valid_fixture_bytes() {
        let first = generate_fixture(0x1234_5678, 17).to_json().unwrap();
        let repeated = generate_fixture(0x1234_5678, 17).to_json().unwrap();
        assert_eq!(first, repeated);
        assert!(Fixture::from_json(&first).is_ok());
        assert!(GENERATOR_VERSION.ends_with("v1"));
    }

    #[test]
    fn register_operation_encoder_places_destination_in_modrm_reg_field() {
        let mut bytes = Vec::new();
        encode_register_operation(&mut bytes, 1, 1, 2);
        assert_eq!(bytes, [0x48, 0x2b, 0xca]);
        let decoded = od_isa::decode(&bytes).unwrap();
        assert!(matches!(
            decoded[0].operation,
            od_isa::Operation::Sub {
                dst: od_isa::Register::Rcx,
                src: od_isa::Register::Rdx
            }
        ));
    }

    #[test]
    fn fixed_seed_generates_one_thousand_distinct_valid_cases() {
        let cases: HashSet<_> = (0..1_000)
            .map(|sequence| {
                let fixture = generate_fixture(0x5eed_cafe, sequence);
                fixture.validate().unwrap();
                fixture.to_json().unwrap()
            })
            .collect();
        assert_eq!(cases.len(), 1_000);
    }
}
