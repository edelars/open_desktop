//! Versioned observations and comparison for one-instruction Long64 paged-memory cases.
//!
//! This module records backend observations; it does not implement instruction or paging
//! semantics. GPR array order is RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8..R15.

use crate::{HexU64, MAX_MEMORY_BYTES, result::Observation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MEMORY_FAULT_SCHEMA_VERSION: u32 = 1;
const GPR_NAMES: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    UncachedInterpreter,
    CachedInterpreter,
    Jit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionStatus {
    Completed {},
    Unsupported { reason: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Completed {},
    PageFault {
        vector: u8,
        pfec: HexU64,
        faulting_rip: HexU64,
        cr2_value: HexU64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryDelta {
    pub physical_address: HexU64,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalWriteRange {
    pub physical_address: HexU64,
    pub length: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceEvent {
    pub device: String,
    pub event: String,
}

/// One normalized backend run for a fixed Long64 paged-memory operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryFaultObservation {
    pub schema_version: u32,
    pub case_id: String,
    pub input_sha256: String,
    pub backend: Backend,
    pub status: ExecutionStatus,
    pub gprs: Observation<[HexU64; 16]>,
    pub rip: Observation<HexU64>,
    pub rflags: Observation<HexU64>,
    pub rflags_defined: HexU64,
    pub retired: Observation<u64>,
    pub outcome: Observation<Outcome>,
    /// Architectural CR2 state, distinct from the page fault's CR2 value.
    pub architectural_cr2: Observation<HexU64>,
    pub memory_deltas: Observation<Vec<MemoryDelta>>,
    /// Physical write ranges in the order callbacks were delivered.
    pub committed_writes: Observation<Vec<PhysicalWriteRange>>,
    pub device_events: Observation<Vec<DeviceEvent>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Difference {
    pub field: String,
    pub expected: String,
    pub actual: String,
    pub physical_address: Option<HexU64>,
    /// XOR mask of differing bits when the difference is a RAM byte.
    pub differing_bits: Option<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompareError {
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    Incompatible(&'static str),
    Unsupported {
        backend: Backend,
        reason: String,
    },
    Missing {
        side: &'static str,
        field: &'static str,
    },
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::Incompatible(field) => write!(f, "observations differ in {field}"),
            Self::Unsupported { backend, reason } => {
                write!(f, "backend {backend:?} is unsupported: {reason}")
            }
            Self::Missing { side, field } => write!(f, "{side} observation is missing {field}"),
        }
    }
}

impl std::error::Error for CompareError {}

impl MemoryFaultObservation {
    pub fn validate(&self) -> Result<(), CompareError> {
        if self.schema_version != MEMORY_FAULT_SCHEMA_VERSION {
            return Err(CompareError::Invalid {
                field: "schema_version",
                reason: "unsupported schema version",
            });
        }
        if self.case_id.is_empty() || self.case_id.len() > 128 {
            return Err(CompareError::Invalid {
                field: "case_id",
                reason: "invalid length",
            });
        }
        if self.input_sha256.len() != 64
            || !self
                .input_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CompareError::Invalid {
                field: "input_sha256",
                reason: "expected lowercase SHA-256 hex",
            });
        }
        if let ExecutionStatus::Unsupported { reason } = &self.status {
            if reason.trim().is_empty() || reason.len() > 512 {
                return Err(CompareError::Invalid {
                    field: "status.reason",
                    reason: "invalid length",
                });
            }
            return Ok(());
        }
        let gprs = required(&self.gprs, "run", "gprs")?;
        let _ = required(&self.rip, "run", "rip")?;
        let _ = required(&self.rflags, "run", "rflags")?;
        let _ = required(&self.retired, "run", "retired")?;
        let outcome = required(&self.outcome, "run", "outcome")?;
        let deltas = required(&self.memory_deltas, "run", "memory_deltas")?;
        let _ = required(&self.committed_writes, "run", "committed_writes")?;
        let _ = required(&self.device_events, "run", "device_events")?;
        if let Outcome::PageFault { vector, .. } = outcome {
            if *vector != 14 {
                return Err(CompareError::Invalid {
                    field: "outcome.vector",
                    reason: "page fault vector must be 14",
                });
            }
        }
        for value in gprs.iter() {
            let _ = value.get();
        }
        let mut total = 0usize;
        let mut end = None;
        for delta in deltas {
            if delta.before.is_empty() || delta.before.len() != delta.after.len() {
                return Err(CompareError::Invalid {
                    field: "memory_deltas",
                    reason: "delta byte arrays must be nonempty and equal length",
                });
            }
            total = total.saturating_add(delta.before.len());
            let start = delta.physical_address.get();
            let next =
                start
                    .checked_add(delta.before.len() as u64)
                    .ok_or(CompareError::Invalid {
                        field: "memory_deltas",
                        reason: "physical range overflows",
                    })?;
            if end.is_some_and(|prior| start < prior) {
                return Err(CompareError::Invalid {
                    field: "memory_deltas",
                    reason: "deltas must be sorted and nonoverlapping",
                });
            }
            end = Some(next);
        }
        if total > MAX_MEMORY_BYTES {
            return Err(CompareError::Invalid {
                field: "memory_deltas",
                reason: "exceeds memory observation limit",
            });
        }
        for range in required(&self.committed_writes, "run", "committed_writes")? {
            if range.length == 0
                || range
                    .physical_address
                    .get()
                    .checked_add(u64::from(range.length))
                    .is_none()
            {
                return Err(CompareError::Invalid {
                    field: "committed_writes",
                    reason: "range must be nonempty and not overflow",
                });
            }
        }
        if let Observation::Unavailable { reason } = &self.architectural_cr2 {
            if reason.trim().is_empty() || reason.len() > 512 {
                return Err(CompareError::Invalid {
                    field: "architectural_cr2.reason",
                    reason: "invalid length",
                });
            }
        }
        Ok(())
    }
}

/// Validate and compare two completed observations. RFLAGS differences outside the shared
/// `rflags_defined` mask are ignored; differing masks are rejected as incompatible.
pub fn compare(
    left: &MemoryFaultObservation,
    right: &MemoryFaultObservation,
) -> Result<Vec<Difference>, CompareError> {
    left.validate()?;
    right.validate()?;
    if left.case_id != right.case_id {
        return Err(CompareError::Incompatible("case_id"));
    }
    if left.input_sha256 != right.input_sha256 {
        return Err(CompareError::Incompatible("input_sha256"));
    }
    if let ExecutionStatus::Unsupported { reason } = &left.status {
        return Err(CompareError::Unsupported {
            backend: left.backend,
            reason: reason.clone(),
        });
    }
    if let ExecutionStatus::Unsupported { reason } = &right.status {
        return Err(CompareError::Unsupported {
            backend: right.backend,
            reason: reason.clone(),
        });
    }
    if left.rflags_defined != right.rflags_defined {
        return Err(CompareError::Incompatible("rflags_defined"));
    }
    let mut differences = Vec::new();
    let _ = required(&left.gprs, "left", "gprs")?;
    let _ = required(&right.gprs, "right", "gprs")?;
    if let (Observation::Available { value: a }, Observation::Available { value: b }) =
        (&left.gprs, &right.gprs)
    {
        for index in 0..16 {
            if a[index] != b[index] {
                differences.push(diff(
                    format!("gprs.{}", GPR_NAMES[index]),
                    format!("0x{:016x}", a[index].get()),
                    format!("0x{:016x}", b[index].get()),
                ));
            }
        }
    }
    compare_observation("rip", &left.rip, &right.rip, &mut differences)?;
    compare_masked_flags(left, right, &mut differences)?;
    compare_observation("retired", &left.retired, &right.retired, &mut differences)?;
    compare_observation("outcome", &left.outcome, &right.outcome, &mut differences)?;
    compare_optional_observation(
        "architectural_cr2",
        &left.architectural_cr2,
        &right.architectural_cr2,
        &mut differences,
    );
    compare_memory(
        required(&left.memory_deltas, "left", "memory_deltas")?,
        required(&right.memory_deltas, "right", "memory_deltas")?,
        &mut differences,
    );
    compare_observation(
        "committed_writes",
        &left.committed_writes,
        &right.committed_writes,
        &mut differences,
    )?;
    compare_observation(
        "device_events",
        &left.device_events,
        &right.device_events,
        &mut differences,
    )?;
    Ok(differences)
}

fn required<'a, T>(
    value: &'a Observation<T>,
    side: &'static str,
    field: &'static str,
) -> Result<&'a T, CompareError> {
    match value {
        Observation::Available { value } => Ok(value),
        Observation::Unavailable { .. } => Err(CompareError::Missing { side, field }),
    }
}

fn compare_observation<T: std::fmt::Debug + PartialEq>(
    field: &'static str,
    a: &Observation<T>,
    b: &Observation<T>,
    out: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let a = required(a, "left", field)?;
    let b = required(b, "right", field)?;
    if a != b {
        out.push(diff(field.to_owned(), format!("{a:?}"), format!("{b:?}")));
    }
    Ok(())
}

fn compare_optional_observation<T: std::fmt::Debug + PartialEq>(
    field: &'static str,
    a: &Observation<T>,
    b: &Observation<T>,
    out: &mut Vec<Difference>,
) {
    match (a, b) {
        (Observation::Available { value: a }, Observation::Available { value: b }) if a != b => {
            out.push(diff(field.into(), format!("{a:?}"), format!("{b:?}")));
        }
        (Observation::Available { .. }, Observation::Unavailable { reason }) => {
            out.push(diff(
                field.into(),
                "available".into(),
                format!("unavailable: {reason}"),
            ));
        }
        (Observation::Unavailable { reason }, Observation::Available { .. }) => {
            out.push(diff(
                field.into(),
                format!("unavailable: {reason}"),
                "available".into(),
            ));
        }
        _ => {}
    }
}

fn compare_masked_flags(
    left: &MemoryFaultObservation,
    right: &MemoryFaultObservation,
    out: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let a = required(&left.rflags, "left", "rflags")?.get();
    let b = required(&right.rflags, "right", "rflags")?.get();
    let mask = left.rflags_defined.get();
    let changed = (a ^ b) & mask;
    if changed != 0 {
        out.push(diff(
            "rflags".into(),
            format!("0x{a:016x}"),
            format!("0x{b:016x}"),
        ));
    }
    Ok(())
}

fn compare_memory(a: &[MemoryDelta], b: &[MemoryDelta], out: &mut Vec<Difference>) {
    let mut bytes_a = BTreeMap::new();
    let mut bytes_b = BTreeMap::new();
    for delta in a {
        for (i, (before, after)) in delta.before.iter().zip(&delta.after).enumerate() {
            bytes_a.insert(delta.physical_address.get() + i as u64, (*before, *after));
        }
    }
    for delta in b {
        for (i, (before, after)) in delta.before.iter().zip(&delta.after).enumerate() {
            bytes_b.insert(delta.physical_address.get() + i as u64, (*before, *after));
        }
    }
    for address in bytes_a
        .keys()
        .chain(bytes_b.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let left = bytes_a.get(&address).copied();
        let right = bytes_b.get(&address).copied();
        if left != right {
            let av = left.map(|(before, after)| (before, after));
            let bv = right.map(|(before, after)| (before, after));
            let bits = match (av, bv) {
                (Some((before_a, after_a)), Some((before_b, after_b))) => {
                    Some((before_a ^ before_b) | (after_a ^ after_b))
                }
                _ => None,
            };
            out.push(Difference {
                field: "memory_deltas".into(),
                expected: format!("{av:?}"),
                actual: format!("{bv:?}"),
                physical_address: Some(HexU64(address)),
                differing_bits: bits,
            });
        }
    }
}

fn diff(field: String, expected: String, actual: String) -> Difference {
    Difference {
        field,
        expected,
        actual,
        physical_address: None,
        differing_bits: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available<T>(value: T) -> Observation<T> {
        Observation::Available { value }
    }
    fn sample(backend: Backend) -> MemoryFaultObservation {
        MemoryFaultObservation {
            schema_version: MEMORY_FAULT_SCHEMA_VERSION,
            case_id: "s4-17a-success".into(),
            input_sha256: "a".repeat(64),
            backend,
            status: ExecutionStatus::Completed {},
            gprs: available([HexU64(0); 16]),
            rip: available(HexU64(0x1003)),
            rflags: available(HexU64(2)),
            rflags_defined: HexU64(0x2),
            retired: available(1),
            outcome: available(Outcome::Completed {}),
            architectural_cr2: Observation::Unavailable {
                reason: "API does not commit CR2".into(),
            },
            memory_deltas: available(vec![MemoryDelta {
                physical_address: HexU64(0x7000),
                before: vec![0x03],
                after: vec![0x03],
            }]),
            committed_writes: available(vec![]),
            device_events: available(vec![]),
        }
    }

    #[test]
    fn memory_difference_identifies_physical_address_and_a_bit() {
        let left = sample(Backend::UncachedInterpreter);
        let mut right = sample(Backend::Jit);
        right.memory_deltas = available(vec![MemoryDelta {
            physical_address: HexU64(0x7000),
            before: vec![0x03],
            after: vec![0x23],
        }]);
        let differences = compare(&left, &right).unwrap();
        let memory = differences
            .iter()
            .find(|d| d.field == "memory_deltas")
            .unwrap();
        assert_eq!(memory.physical_address, Some(HexU64(0x7000)));
        assert_eq!(memory.differing_bits, Some(0x20));
    }

    #[test]
    fn undefined_flags_are_masked_but_defined_flags_mismatch() {
        let left = sample(Backend::UncachedInterpreter);
        let mut right = sample(Backend::Jit);
        right.rflags = available(HexU64(0x202));
        assert!(compare(&left, &right).unwrap().is_empty());
        right.rflags = available(HexU64(0x200));
        assert!(
            compare(&left, &right)
                .unwrap()
                .iter()
                .any(|d| d.field == "rflags")
        );
    }

    #[test]
    fn missing_required_observation_is_rejected() {
        let left = sample(Backend::UncachedInterpreter);
        let mut right = sample(Backend::Jit);
        right.device_events = Observation::Unavailable {
            reason: "not captured".into(),
        };
        assert!(matches!(
            compare(&left, &right),
            Err(CompareError::Missing {
                side: "run",
                field: "device_events"
            })
        ));
    }

    #[test]
    fn cached_interpreter_can_be_explicitly_unsupported() {
        let mut run = sample(Backend::CachedInterpreter);
        run.status = ExecutionStatus::Unsupported {
            reason: "tracked cached execution unavailable".into(),
        };
        assert!(run.validate().is_ok());
        assert!(matches!(
            compare(&sample(Backend::UncachedInterpreter), &run),
            Err(CompareError::Unsupported {
                backend: Backend::CachedInterpreter,
                ..
            })
        ));
    }

    #[test]
    fn json_rejects_unknown_fields_and_round_trips() {
        let run = sample(Backend::UncachedInterpreter);
        let json = serde_json::to_vec(&run).unwrap();
        assert_eq!(
            serde_json::from_slice::<MemoryFaultObservation>(&json).unwrap(),
            run
        );
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["surprise"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MemoryFaultObservation>(value).is_err());
    }
}
