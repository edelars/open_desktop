use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use od_isa::long_decode::{self, AddressBase, MemoryOperand, Operand, Operation};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_isa::{OperandSize, Register, RegisterView};
use serde_json::Value;

pub(crate) fn decode_one(code: &[u8]) -> Result<long_decode::Instruction, String> {
    struct CodeMemory<'a> {
        bytes: &'a [u8],
    }
    impl GuestMemory for CodeMemory<'_> {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.bytes
                .get(address as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("decode-only memory".into()))
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("decode-only memory".into()))
        }
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend("decode-only memory".into()))
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend("decode-only memory".into()))
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend("decode-only memory".into()))
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("decode-only memory".into()))
        }
    }
    let mut memory = CodeMemory { bytes: code };
    let instruction = long_decode::decode(&mut memory, 0, AccessContext::default())
        .map_err(|e| format!("od-isa rejected corpus instruction: {e:?}"))?;
    Ok(instruction)
}

pub(crate) fn validate_decoded_litmus_instruction(
    instruction: &Value,
    decoded: long_decode::Instruction,
) -> Result<(), String> {
    let op = instruction
        .get("op")
        .and_then(Value::as_str)
        .ok_or("instruction missing op")?;
    let operands = instruction
        .get("operands")
        .ok_or("instruction missing operands")?;
    let operand_names: &[&str] = match op {
        "store" => &["address", "value"],
        "load" => &["address", "destination"],
        "mfence" => &[],
        "locked_rmw" | "implicit_locked_rmw" => &["address"],
        "rmw" => &["address", "delta"],
        "register_store" => &["address", "source"],
        "register_alu" => &["destination", "source"],
        "loop" => &["counter", "relative_displacement", "target"],
        _ => return Err(format!("unsupported litmus operation {op}")),
    };
    if operand_names
        .iter()
        .any(|name| operands.get(name).is_none_or(|value| value.is_null()))
    {
        return Err(format!("instruction {op} is missing a required operand"));
    }
    let forbidden_operand: &[&str] = match op {
        "store" => &["register"],
        "load" => &["value", "register"],
        "mfence" => &["address", "value", "register", "destination"],
        "register_alu" => &["address", "value", "register"],
        "locked_rmw" | "implicit_locked_rmw" => &["value", "destination"],
        "rmw" => &["value", "register", "destination"],
        "register_store" => &["value", "register"],
        "loop" => &["address", "value", "register", "destination"],
        _ => &[],
    };
    if forbidden_operand
        .iter()
        .any(|name| operands.get(name).is_some())
    {
        return Err(format!("instruction {op} has an extra operand"));
    }
    let declared_address = operands
        .get("address")
        .and_then(Value::as_str)
        .map(parse_u32)
        .transpose()?;
    let expected_prefixes = if op == "locked_rmw" {
        long_decode::Prefixes {
            group1: Some(0xf0),
            ..long_decode::Prefixes::default()
        }
    } else {
        long_decode::Prefixes::default()
    };
    if decoded.prefixes != expected_prefixes {
        return Err(format!("instruction {op} uses unsupported prefixes"));
    }
    let memory_matches = |memory: MemoryOperand| {
        declared_address.is_some_and(|address| {
            memory.base == AddressBase::None
                && memory.index.is_none()
                && memory.scale == 1
                && memory.displacement == i64::from(address as i32)
                && memory.address_size == OperandSize::Qword
                && memory.size == OperandSize::Dword
        })
    };
    let register_matches = |view: RegisterView, name: &str| {
        expected_register_view(name).is_some_and(|expected| view == expected)
    };
    let valid = match op {
        "store" => match decoded.operation {
            Operation::MovImmediate {
                dst: Operand::Memory(memory),
                value,
            } => {
                memory_matches(memory)
                    && operands.get("value").and_then(Value::as_u64) == Some(value)
            }
            _ => false,
        },
        "load" => match decoded.operation {
            Operation::Mov {
                dst: Operand::Register(destination),
                src: Operand::Memory(memory),
            } => {
                operands
                    .get("destination")
                    .and_then(Value::as_str)
                    .is_some_and(|name| register_matches(destination, name))
                    && memory_matches(memory)
            }
            _ => false,
        },
        "mfence" => matches!(decoded.operation, Operation::Mfence),
        "locked_rmw" => match decoded.operation {
            Operation::LockIncMem { destination } => {
                memory_matches(destination) && operands.get("register").is_none()
            }
            Operation::LockXaddMem {
                destination,
                source,
            } => {
                memory_matches(destination)
                    && operands
                        .get("register")
                        .and_then(Value::as_str)
                        .is_some_and(|name| register_matches(source, name))
            }
            Operation::LockCmpxchgMem {
                destination,
                source,
            } => {
                memory_matches(destination)
                    && operands
                        .get("register")
                        .and_then(Value::as_str)
                        .is_some_and(|name| register_matches(source, name))
            }
            _ => false,
        },
        "implicit_locked_rmw" => match decoded.operation {
            Operation::XchgMemory { register, memory } => {
                memory_matches(memory)
                    && operands
                        .get("register")
                        .and_then(Value::as_str)
                        .is_some_and(|name| register_matches(register, name))
            }
            _ => false,
        },
        "rmw" => match decoded.operation {
            Operation::Arithmetic {
                kind: long_decode::ArithmeticKind::Inc,
                dst: Operand::Memory(memory),
                src: Operand::Immediate(1),
            } => memory_matches(memory) && operands.get("delta").and_then(Value::as_u64) == Some(1),
            _ => false,
        },
        "register_alu" => match decoded.operation {
            Operation::Arithmetic {
                kind: long_decode::ArithmeticKind::Add,
                dst: Operand::Register(destination),
                src: Operand::Immediate(1),
            } => {
                register_matches(destination, "eax")
                    && operands.get("destination").and_then(Value::as_str) == Some("eax")
                    && operands.get("source").and_then(Value::as_u64) == Some(1)
            }
            _ => false,
        },
        "register_store" => match decoded.operation {
            Operation::Mov {
                dst: Operand::Memory(memory),
                src: Operand::Register(source),
            } => {
                memory_matches(memory)
                    && register_matches(source, "eax")
                    && operands.get("source").and_then(Value::as_str) == Some("eax")
            }
            _ => false,
        },
        "loop" => match decoded.operation {
            Operation::Loop { displacement } => {
                operands.get("counter").and_then(Value::as_str) == Some("rcx")
                    && operands
                        .get("relative_displacement")
                        .and_then(Value::as_i64)
                        == Some(i64::from(displacement))
                    && operands.get("target").and_then(Value::as_str) == Some(".loop")
            }
            _ => false,
        },
        _ => false,
    };
    if !valid {
        return Err(format!(
            "instruction form {op} is outside the dedicated S6-05 litmus admission"
        ));
    }
    if matches!(
        op,
        "store" | "load" | "locked_rmw" | "implicit_locked_rmw" | "rmw" | "register_store"
    ) && declared_address.is_none()
    {
        return Err(format!(
            "memory instruction {op} has no declared address operand"
        ));
    }
    Ok(())
}

fn expected_register_view(name: &str) -> Option<RegisterView> {
    let register = match name {
        "eax" => Register::Rax,
        "ecx" => Register::Rcx,
        "edx" => Register::Rdx,
        "ebx" => Register::Rbx,
        _ => return None,
    };
    Some(RegisterView::low(register, OperandSize::Dword))
}

pub(crate) fn parse_u32(text: &str) -> Result<u32, String> {
    let normalized = text.trim();
    let value = if let Some(value) = normalized.strip_prefix("0x") {
        u32::from_str_radix(value, 16)
    } else {
        normalized.parse::<u32>()
    };
    value.map_err(|_| format!("invalid u32 address {text}"))
}

fn absolute_disp32_offsets(bytes: &[u8], address: u32) -> Vec<usize> {
    let needle = address.to_le_bytes();
    bytes
        .windows(4)
        .enumerate()
        .filter_map(|(offset, window)| {
            let valid_absolute_sib =
                offset >= 2 && bytes[offset - 2] & 0xc7 == 0x04 && bytes[offset - 1] & 0x07 == 0x05;
            (valid_absolute_sib && window == needle).then_some(offset)
        })
        .collect()
}

fn validate_event_contract(instruction: &Value, cells: &BTreeSet<String>) -> Result<(), String> {
    let op = instruction
        .get("op")
        .and_then(Value::as_str)
        .ok_or("missing op")?;
    let bytes = instruction
        .get("bytes")
        .and_then(Value::as_str)
        .ok_or("missing bytes")?;
    let encoded = parse_hex(bytes)?;
    let decoded = decode_one(&encoded)?;
    let expected: &[&str] = match op {
        "store" | "register_store" => &["write"],
        "load" => &["read"],
        "rmw" => &["rmw_read", "rmw_write"],
        "mfence" | "register_alu" | "loop" => &[],
        "locked_rmw" => match decoded.operation {
            Operation::LockIncMem { .. } => &["atomic_rmw"],
            Operation::LockXaddMem { .. } => &["atomic_xadd"],
            Operation::LockCmpxchgMem { .. } => &["atomic_cmpxchg"],
            _ => return Err("locked_rmw does not decode to a supported locked operation".into()),
        },
        "implicit_locked_rmw" => &["atomic_xchg"],
        _ => return Err(format!("unsupported event op {op}")),
    };
    let events = instruction
        .get("memory_events")
        .and_then(Value::as_array)
        .ok_or("missing memory_events array")?;
    let mut actual = events
        .iter()
        .map(|event| {
            let location = event
                .get("location")
                .and_then(Value::as_str)
                .ok_or("event missing location")?;
            if !cells.contains(location) {
                return Err(format!("event names undeclared memory cell {location}"));
            }
            event
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| "event missing kind".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    if actual != expected {
        return Err(format!(
            "{op} decoded side effects disagree with memory_events"
        ));
    }
    if let Some(address) = instruction
        .get("operands")
        .and_then(|operands| operands.get("address"))
        .and_then(Value::as_str)
    {
        let offsets = absolute_disp32_offsets(&encoded, parse_u32(address)?);
        if offsets.len() != 1 {
            return Err(format!(
                "declared address has {} encoded disp32 fields",
                offsets.len()
            ));
        }
    }
    if matches!(
        op,
        "store" | "load" | "locked_rmw" | "implicit_locked_rmw" | "rmw" | "register_store"
    ) && instruction
        .get("operands")
        .and_then(|operands| operands.get("address"))
        .is_none()
    {
        return Err(format!("memory operation {op} has no address"));
    }
    Ok(())
}

fn parse_hex(bytes: &str) -> Result<Vec<u8>, String> {
    bytes
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).map_err(|_| format!("invalid byte {byte}")))
        .collect()
}

pub(crate) fn validate_observation_contract(
    fields: &[String],
    expected: &BTreeSet<Vec<u64>>,
    cells: &BTreeSet<String>,
    thread_count: usize,
) -> Result<(), String> {
    if fields.is_empty() || expected.is_empty() {
        return Err("observation fields and expected outcomes must be nonempty".into());
    }
    if fields.iter().collect::<BTreeSet<_>>().len() != fields.len() {
        return Err("observe.fields contains duplicates".into());
    }
    for field in fields {
        if let Some(rest) = field.strip_prefix("cpu") {
            let (cpu, register) = rest.split_once('.').ok_or("malformed CPU observation")?;
            let cpu = cpu
                .parse::<usize>()
                .map_err(|_| "malformed CPU observation")?;
            if cpu >= thread_count || !matches!(register, "eax" | "ebx" | "ecx" | "edx") {
                return Err(format!("unsupported register observation {field}"));
            }
        } else if let Some(cell) = field.strip_prefix("mem.") {
            if !cells.contains(cell) {
                return Err(format!("observation names missing cell {cell}"));
            }
        } else {
            return Err(format!("unsupported observation field {field}"));
        }
    }
    if expected.iter().any(|outcome| outcome.len() != fields.len()) {
        return Err("expected outcome width differs from observe.fields".into());
    }
    if expected
        .iter()
        .flatten()
        .any(|value| *value > u32::MAX as u64)
    {
        return Err("expected values exceed the admitted 32-bit observation width".into());
    }
    Ok(())
}

pub(crate) fn operation_writes_rcx(operation: Operation) -> bool {
    let is_rcx = |view: RegisterView| view.register() == Register::Rcx;
    match operation {
        Operation::MovImmediate {
            dst: Operand::Register(destination),
            ..
        }
        | Operation::Mov {
            dst: Operand::Register(destination),
            ..
        }
        | Operation::Arithmetic {
            dst: Operand::Register(destination),
            ..
        } => is_rcx(destination),
        Operation::LockXaddMem { source, .. } => is_rcx(source),
        Operation::XchgMemory { register, .. } => is_rcx(register),
        _ => false,
    }
}

pub(crate) fn select_affinity_by_topology(
    allowed: &[usize],
    workers: usize,
    topology: &BTreeMap<usize, (String, String)>,
) -> Result<Vec<usize>, String> {
    let mut allowed = allowed.to_vec();
    allowed.sort_unstable();
    allowed.dedup();
    if allowed.len() < workers {
        return Err(format!(
            "requires {workers} distinct allowed logical CPUs; only {} are available",
            allowed.len()
        ));
    }
    let mut selected = Vec::new();
    let mut seen_cores = BTreeSet::new();
    for cpu in &allowed {
        let (package, core) = topology
            .get(cpu)
            .ok_or_else(|| format!("CPU topology unavailable for logical CPU {cpu}"))?;
        if seen_cores.insert((package.clone(), core.clone())) {
            selected.push(*cpu);
            if selected.len() == workers {
                return Ok(selected);
            }
        }
    }
    for cpu in allowed {
        if !selected.contains(&cpu) {
            selected.push(cpu);
            if selected.len() == workers {
                return Ok(selected);
            }
        }
    }
    Err("could not select distinct allowed logical CPUs".into())
}

pub(crate) fn validate_histogram_total(
    histogram: &BTreeMap<String, u64>,
    expected: u64,
) -> Result<u64, String> {
    let observed = histogram.values().copied().sum::<u64>();
    if observed != expected {
        return Err(format!(
            "histogram sum {observed} differs from requested rounds {expected}"
        ));
    }
    Ok(observed)
}

pub(crate) fn validate_low_mapping(base: usize, len: usize) -> Result<(), String> {
    if len == 0
        || base >= i32::MAX as usize
        || base
            .checked_add(len)
            .is_none_or(|end| end >= i32::MAX as usize)
    {
        return Err("mapping is outside positive signed disp32 reach".into());
    }
    Ok(())
}

pub(crate) fn valid_corpus_inventory(
    finite_ids: &BTreeSet<String>,
    formula_ids: &BTreeSet<String>,
    execution_count: usize,
    formula_count: usize,
) -> bool {
    let expected_finite = [
        "2+2W",
        "IRIW",
        "LB",
        "LOCK-CMPXCHG-MUTEX",
        "LOCK-XADD",
        "MP",
        "SB-LOCK-LOCAL",
        "SB-LOCK-XADD",
        "SB-LOCK",
        "SB-MFENCE",
        "SB-XCHG",
        "SB",
        "STORE-FORWARDING",
        "UNLOCKED-INC",
        "XCHG-MUTEX",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    let expected_formula = ["LOCK-INC", "THREE-INSN-COUNTER", "UNLOCKED-INC"]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    finite_ids == &expected_finite
        && formula_ids == &expected_formula
        && formula_count == expected_formula.len()
        && execution_count == expected_finite.len() + expected_formula.len()
}

pub(crate) fn execution_id(kind: &str, fixture_id: &str, n: usize, k: u64) -> String {
    format!("{kind}:{fixture_id}:n{n}:k{k}")
}

pub(crate) fn required_output_path(args: &[String]) -> Result<PathBuf, String> {
    if args.len() != 10 || args.get(8).map(String::as_str) != Some("--output") {
        return Err("--output RESULT.json is required for native litmus runs".into());
    }
    let path = args
        .get(9)
        .filter(|path| !path.is_empty())
        .ok_or("--output path is empty")?;
    Ok(PathBuf::from(path))
}

pub(crate) fn decode_build_metadata(value: &str) -> Option<String> {
    if value.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn affinity_uses_distinct_cores_before_smt_siblings_and_fails_closed() {
        let topology = BTreeMap::from([
            (1, ("0".into(), "0".into())),
            (2, ("0".into(), "0".into())),
            (3, ("0".into(), "1".into())),
        ]);
        assert!(select_affinity_by_topology(&[2, 2], 2, &topology).is_err());
        assert_eq!(
            select_affinity_by_topology(&[3, 1, 2], 2, &topology).unwrap(),
            vec![1, 3]
        );
        assert_eq!(
            select_affinity_by_topology(&[2, 1], 2, &topology).unwrap(),
            vec![1, 2]
        );
        let four_threads = BTreeMap::from([
            (0, ("0".into(), "0".into())),
            (1, ("0".into(), "0".into())),
            (2, ("0".into(), "1".into())),
            (3, ("0".into(), "1".into())),
        ]);
        let selected = select_affinity_by_topology(&[0, 1, 2, 3], 4, &four_threads).unwrap();
        assert_eq!(selected, vec![0, 2, 1, 3]);
        let distinct_core_count = selected
            .iter()
            .map(|cpu| four_threads[cpu].clone())
            .collect::<BTreeSet<_>>()
            .len();
        assert_eq!(distinct_core_count, 2);
        assert!(
            selected.len() > distinct_core_count,
            "SMT siblings are selected only after all physical cores"
        );
    }

    #[test]
    fn corpus_inventory_and_identity_preserve_finite_formula_collision() {
        let finite = [
            "2+2W",
            "IRIW",
            "LB",
            "LOCK-CMPXCHG-MUTEX",
            "LOCK-XADD",
            "MP",
            "SB-LOCK-LOCAL",
            "SB-LOCK-XADD",
            "SB-LOCK",
            "SB-MFENCE",
            "SB-XCHG",
            "SB",
            "STORE-FORWARDING",
            "UNLOCKED-INC",
            "XCHG-MUTEX",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
        let formula = ["LOCK-INC", "THREE-INSN-COUNTER", "UNLOCKED-INC"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert!(finite.contains("UNLOCKED-INC") && formula.contains("UNLOCKED-INC"));
        assert!(valid_corpus_inventory(&finite, &formula, 18, 3));
        assert!(!valid_corpus_inventory(&finite, &formula, 17, 3));
        assert_ne!(
            execution_id("finite", "UNLOCKED-INC", 2, 2),
            execution_id("formula", "UNLOCKED-INC", 2, 2)
        );
    }

    #[test]
    fn histogram_requires_exact_fixed_round_count() {
        let histogram = BTreeMap::from([("0,1".to_string(), 2), ("1,0".to_string(), 3)]);
        assert_eq!(validate_histogram_total(&histogram, 5), Ok(5));
        assert!(validate_histogram_total(&histogram, 6).is_err());
    }

    #[test]
    fn native_litmus_cannot_start_without_an_evidence_output_path() {
        assert!(required_output_path(&["corpus".into()]).is_err());
        let valid = vec![
            "d".into(),
            "--iterations".into(),
            "1".into(),
            "--counter-n".into(),
            "2".into(),
            "--counter-k".into(),
            "2".into(),
            "--json".into(),
            "--output".into(),
            "result.json".into(),
        ];
        assert_eq!(
            required_output_path(&valid).unwrap(),
            PathBuf::from("result.json")
        );
    }

    #[test]
    fn build_metadata_hex_decodes_equals_and_multiline_values_without_loss() {
        for value in ["--cfg=feature=fast", "-C opt-level=2\n--cfg=mode=litmus"] {
            let encoded = value
                .bytes()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(decode_build_metadata(&encoded).as_deref(), Some(value));
        }
        assert_eq!(decode_build_metadata("0"), None);
        assert_eq!(decode_build_metadata("zz"), None);
    }

    #[test]
    fn build_script_embeds_source_toolchain_and_explicit_flags_provenance() {
        assert!(!env!("OD_X86_BUILD_SOURCE_REVISION").is_empty());
        assert_eq!(env!("OD_X86_BUILD_SOURCE_SHA256").len(), 64);
        assert!(matches!(
            env!("OD_X86_BUILD_SOURCE_DIRTY"),
            "true" | "false" | "unknown"
        ));
        assert!(decode_build_metadata(env!("OD_X86_BUILD_RUSTC_HEX")).is_some());
        assert!(!env!("OD_X86_BUILD_TARGET").is_empty());
        assert!(!env!("OD_X86_BUILD_PROFILE").is_empty());
        assert!(matches!(
            env!("OD_X86_BUILD_RUSTFLAGS_SOURCE"),
            "RUSTFLAGS" | "CARGO_ENCODED_RUSTFLAGS" | "none"
        ));
        assert!(decode_build_metadata(env!("OD_X86_BUILD_RUSTFLAGS_HEX")).is_some());
    }

    #[test]
    fn relocation_mapping_fails_closed_outside_positive_disp32_range() {
        assert!(validate_low_mapping(0x1000, 4096).is_ok());
        assert!(validate_low_mapping(i32::MAX as usize, 4096).is_err());
        assert!(validate_low_mapping((i32::MAX as usize) - 32, 64).is_err());
    }

    #[test]
    fn malformed_observation_contract_fails_before_worker_creation() {
        let cells = BTreeSet::from(["x".to_string()]);
        let outcomes = BTreeSet::from([vec![0]]);
        assert!(validate_observation_contract(&[], &outcomes, &cells, 2).is_err());
        assert!(validate_observation_contract(&["cpu9.eax".into()], &outcomes, &cells, 2).is_err());
        assert!(
            validate_observation_contract(&["mem.missing".into()], &outcomes, &cells, 2).is_err()
        );
        assert!(
            validate_observation_contract(&["mem.x".into()], &BTreeSet::new(), &cells, 2).is_err()
        );
        assert!(validate_observation_contract(&["mem.x".into()], &outcomes, &cells, 2).is_ok());
    }

    #[test]
    fn formula_body_register_write_classifier_covers_rcx_destination_and_xadd_source() {
        let mov_rcx = decode_one(&[0xb9, 1, 0, 0, 0]).unwrap();
        assert!(operation_writes_rcx(mov_rcx.operation));
        let mov_eax = decode_one(&[0xb8, 1, 0, 0, 0]).unwrap();
        assert!(!operation_writes_rcx(mov_eax.operation));
    }

    #[test]
    fn decoded_admission_rejects_index_register_rsp_push_wrong_operation_and_extra_operand() {
        let cases = [
            (
                "load",
                json!({"address":"0x1000", "destination":"eax"}),
                &[0x8b, 0x04, 0x05, 0x00, 0x10, 0, 0][..],
            ),
            (
                "load",
                json!({"address":"0x1000", "destination":"eax"}),
                &[0x8b, 0x24, 0x25, 0x00, 0x10, 0, 0][..],
            ),
            (
                "locked_rmw",
                json!({"address":"0x1000"}),
                &[0xff, 0x34, 0x25, 0x00, 0x10, 0, 0][..],
            ),
            (
                "rmw",
                json!({"address":"0x1000", "delta":1}),
                &[0xff, 0x0c, 0x25, 0x00, 0x10, 0, 0][..],
            ),
            (
                "load",
                json!({"address":"0x1000", "destination":"eax"}),
                &[0x8b, 0x1c, 0x25, 0x00, 0x10, 0, 0][..],
            ),
            (
                "load",
                json!({"address":"0x1000", "destination":"eax", "register":"ebx"}),
                &[0x8b, 0x04, 0x25, 0x00, 0x10, 0, 0][..],
            ),
        ];
        for (op, operands, bytes) in cases {
            let decoded = decode_one(bytes).unwrap();
            let instruction = json!({"op":op, "operands":operands});
            assert!(
                validate_decoded_litmus_instruction(&instruction, decoded).is_err(),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn counter_templates_preserve_n2_k2_formula_and_instruction_atomic_oracles() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/guest/stage6/litmus/counters-formula.json");
        let group: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for case in group["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let oracle = &case["formula_oracle"];
            if id == "LOCK-INC" {
                assert_eq!(oracle["x86_tso"]["kind"], "exact_singleton");
                assert_eq!(oracle["x86_tso"]["value"], "N*K");
                let instruction = &case["thread_template"]["instructions"][0];
                assert!(matches!(
                    decode_one(&parse_hex(instruction["bytes"].as_str().unwrap()).unwrap())
                        .unwrap()
                        .operation,
                    Operation::LockIncMem { .. }
                ));
                assert_eq!(instruction["memory_events"][0]["kind"], "atomic_rmw");
            } else {
                assert_eq!(oracle["x86_tso"]["kind"], "exact_integer_range");
                assert_eq!(oracle["x86_tso"]["minimum"], "1 if K == 1 else 2");
                assert_eq!(oracle["x86_tso"]["maximum"], "N*K");
            }
            if id == "UNLOCKED-INC" {
                let instruction = &case["thread_template"]["instructions"][0];
                assert!(matches!(
                    decode_one(&parse_hex(instruction["bytes"].as_str().unwrap()).unwrap())
                        .unwrap()
                        .operation,
                    Operation::Arithmetic {
                        kind: long_decode::ArithmeticKind::Inc,
                        ..
                    }
                ));
                assert_eq!(instruction["memory_events"].as_array().unwrap().len(), 2);
                assert_eq!(
                    oracle["interleaving_instruction_atomic"]["kind"],
                    "exact_singleton"
                );
                assert_eq!(oracle["interleaving_instruction_atomic"]["value"], "N*K");
            }
            if id == "THREE-INSN-COUNTER" {
                assert_eq!(
                    oracle["interleaving_instruction_atomic"]["kind"],
                    "exact_integer_range"
                );
                assert_eq!(oracle["interleaving_instruction_atomic"]["maximum"], "N*K");
                let operations = case["thread_template"]["instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|instruction| instruction["op"].as_str().unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(
                    operations,
                    ["load", "register_alu", "register_store", "loop"]
                );
            }
        }
        let total = 2_u64 * 2;
        assert_eq!(total, 4);
        let cases = group["cases"].as_array().unwrap();
        let locked = cases.iter().find(|case| case["id"] == "LOCK-INC").unwrap();
        assert_eq!(locked["formula_oracle"]["x86_tso"]["value"], "N*K");
        let unlocked = cases
            .iter()
            .find(|case| case["id"] == "UNLOCKED-INC")
            .unwrap();
        assert_eq!(
            unlocked["formula_oracle"]["x86_tso"]["minimum"],
            "1 if K == 1 else 2"
        );
        assert_eq!(
            unlocked["formula_oracle"]["interleaving_instruction_atomic"]["value"],
            "N*K"
        );
        let three = cases
            .iter()
            .find(|case| case["id"] == "THREE-INSN-COUNTER")
            .unwrap();
        assert_eq!(
            three["formula_oracle"]["x86_tso"]["minimum"],
            "1 if K == 1 else 2"
        );
        assert_eq!(
            three["formula_oracle"]["interleaving_instruction_atomic"]["minimum"],
            "1 if K == 1 else 2"
        );
    }

    #[test]
    fn full_corpus_decodes_and_matches_declared_memory_effects() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/guest/stage6/litmus");
        let mut paths = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 16);
        let mut fixture_count = 0;
        let mut instruction_count = 0;
        let mut loop_count = 0;
        for path in paths {
            let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let cases = value.get("cases").and_then(Value::as_array);
            let records = cases.cloned().unwrap_or_else(|| vec![value]);
            for fixture in records {
                fixture_count += 1;
                let names = fixture["memory"]["cells"]
                    .as_array()
                    .or_else(|| fixture["initial_memory"]["cells"].as_array())
                    .unwrap()
                    .iter()
                    .map(|cell| cell["name"].as_str().unwrap().to_string())
                    .collect::<BTreeSet<_>>();
                let instruction_sets: Vec<&Vec<Value>> =
                    if let Some(threads) = fixture["threads"].as_array() {
                        threads
                            .iter()
                            .map(|thread| thread["instructions"].as_array().unwrap())
                            .collect()
                    } else {
                        vec![
                            fixture["thread_template"]["instructions"]
                                .as_array()
                                .unwrap(),
                        ]
                    };
                for instructions in instruction_sets {
                    for instruction in instructions {
                        instruction_count += 1;
                        let encoded = parse_hex(instruction["bytes"].as_str().unwrap()).unwrap();
                        let decoded = decode_one(&encoded).unwrap();
                        assert_eq!(usize::from(decoded.length), encoded.len());
                        validate_decoded_litmus_instruction(instruction, decoded)
                            .unwrap_or_else(|error| panic!("{}: {error}", fixture["id"]));
                        validate_event_contract(instruction, &names).unwrap_or_else(|error| {
                            panic!("{} {}: {error}", fixture["id"], instruction["op"])
                        });
                        if instruction["op"] == "loop" {
                            loop_count += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(fixture_count, 18);
        assert_eq!(instruction_count, 70);
        assert_eq!(loop_count, 3);
    }
}
