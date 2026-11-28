//! S7-32, ADR-0023 slice C1, plan step C1-0: the CPL0 baseline of the set over the reduced encoding
//! sweep (`cpl3_sweep::encodings`). C1 must keep the CPL0 behaviour of the set (plan §4 item 5):
//! every group (map, opcode, prefix set) keeps the digest recorded on G0 (`ad625e3`, before any C1
//! change of `src/`), except the groups of [`INTENTIONAL_GROUPS`], each with its reason.
//!
//! State variants on CPL0: IOPL {0, 3} × CR4 {`0x6f8`, `0x6f8 & !DE`}. One digest per group: the
//! FNV-1a fold of the outcome digests of its encodings (byte order) × the variants.
//!
//! - `write_cpl0_baseline` (ignored; `OD_WRITE_C1_BASELINE=1`): writes `tests/data/c1_cpl0_baseline.txt`
//!   and the expanded outcome of every encoding and variant to `work/s7-32/c1/base/cpl0-outcomes.txt`.
//! - `cpl0_outcomes_match_the_recorded_baseline`: compares and names the differing groups.
//!   `OD_DUMP_C1_GROUP=<group>` (e.g. `OD_DUMP_C1_GROUP='0F 20 []'`) prints the expanded outcomes of
//!   that group on the current code, in the format of the outcome file, for a comparison with it.

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, CR4_DE, changed_fields, digest_fields, encodings_with_groups, fnv1a, initial_fields,
    machine, outcome_fields, parallel_map_with, run_cloned,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// (IOPL, CR4) of the CPL0 variants, in digest order.
const VARIANTS: [(u8, u64); 4] = [(0, CR4), (0, CR4 & !CR4_DE), (3, CR4), (3, CR4 & !CR4_DE)];

/// The encodings C1 changes on purpose (plan C1-0 item 6), each with its reason: fixed on G0, excluded
/// from the group digests when the baseline is written and when it is compared (their count is
/// printed). A group that gains or loses an encoding outside these predicates differs.
const INTENTIONAL_ENCODINGS: &[(&str, &str, fn(&[u8]) -> bool)] = &[
    (
        "0F 00 /4, /5 refused on G0 or new",
        "C1-5c: VERR (0F 00 /4) and the VERW forms (0F 00 /5) without VerwGdt/VerwMem decode as \
         DescriptorQuery (not decoded on G0); of the narrow forms only the two G0 refused change: \
         `0F 00 2D disp32` (VerwMem with a TI = 1 selector in the pattern: ZF = 0 now) and \
         `0F 00 E8` (VerwGdt, CPL3-only on G0); the seven VerwMem forms G0 executed are compared",
        |bytes| {
            escape_0f(bytes).is_some_and(|(opcode, modrm, _)| {
                opcode == 0x00 && matches!(modrm.map(|m| (m >> 3) & 7), Some(4 | 5))
            }) && (matches!(
                bytes,
                [0x0f, 0x00, 0x2d, 0x40, 0, 0, 0] | [0x0f, 0x00, 0xe8]
            ) || cpl3_sweep::decode(bytes).is_ok_and(|instruction| {
                format!("{:?}", instruction.operation).starts_with("DescriptorQuery")
            }))
        },
    ),
    (
        "0F 02, 0F 03",
        "C1-5c: LAR (0F 02 /r) and LSL (0F 03 /r) are decoded and executed",
        |bytes| escape_0f(bytes).is_some_and(|(opcode, _, _)| matches!(opcode, 0x02 | 0x03)),
    ),
    (
        "0F 20-23 reserved CRn/DRn",
        "C1-2 item 1a: MOV to/from a reserved CRn (CR1, CR5-CR7, CR9-CR15) or DRn with REX.R \
         (DR8-DR15) is DecodeError::Invalid (#UD) at any CPL",
        |bytes| {
            escape_0f(bytes).is_some_and(|(opcode, modrm, rex_r)| {
                let Some(modrm) = modrm else { return false };
                let index = ((modrm >> 3) & 7) | (u8::from(rex_r) << 3);
                match opcode {
                    0x20 | 0x22 => !matches!(index, 0 | 2 | 3 | 4 | 8),
                    0x21 | 0x23 => rex_r,
                    _ => false,
                }
            })
        },
    ),
];

/// The groups C1 changes on purpose as a whole (plan C1-0 item 6): a group name starts with one of
/// these prefixes; its comparison is skipped and printed. Empty on G0; an entry needs its reason in
/// the slice report.
const INTENTIONAL_GROUPS: &[(&str, &str)] = &[];

/// `(opcode, ModRM, REX.R)` of a two-byte-map (`0F xx`, not `0F 38`/`0F 3A`) encoding after its
/// legacy prefixes and REX; `None` for any other encoding.
fn escape_0f(bytes: &[u8]) -> Option<(u8, Option<u8>, bool)> {
    let mut index = 0;
    while index < bytes.len()
        && matches!(
            bytes[index],
            0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3
        )
    {
        index += 1;
    }
    let mut rex_r = false;
    if index < bytes.len() && bytes[index] & 0xf0 == 0x40 {
        rex_r = bytes[index] & 4 != 0;
        index += 1;
    }
    if bytes.get(index) != Some(&0x0f) {
        return None;
    }
    let opcode = *bytes.get(index + 1)?;
    if matches!(opcode, 0x38 | 0x3a) {
        return None;
    }
    Some((opcode, bytes.get(index + 2).copied(), rex_r))
}

fn intentional_encoding(bytes: &[u8]) -> bool {
    INTENTIONAL_ENCODINGS
        .iter()
        .any(|(_, _, predicate)| predicate(bytes))
}

fn intentional(group: &str) -> Option<&'static str> {
    INTENTIONAL_GROUPS
        .iter()
        .find(|(prefix, _)| group.starts_with(prefix))
        .map(|(_, reason)| *reason)
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The directory of the data file: `tests/data`, or `OD_C1_BASELINE_DIR` (the G0 checkout writes the
/// file of the working tree).
fn data_dir() -> PathBuf {
    std::env::var_os("OD_C1_BASELINE_DIR")
        .map_or_else(|| manifest().join("tests/data"), PathBuf::from)
}

fn baseline_path() -> PathBuf {
    data_dir().join("c1_cpl0_baseline.txt")
}

fn outcomes_path() -> PathBuf {
    data_dir().join("../../../../work/s7-32/c1/base/cpl0-outcomes.txt")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One encoding: the outcome digest per variant and, when `expanded`, the dump lines.
struct EncodingOutcome {
    digests: [u64; 4],
    lines: Option<String>,
}

/// Every group with its encodings (byte order) and, per encoding, the outcomes.
struct Sweep {
    groups: BTreeMap<String, Vec<(Vec<u8>, EncodingOutcome)>>,
    encodings: usize,
}

fn sweep(expanded: impl Fn(&str) -> bool + Sync) -> Sweep {
    let encodings: Vec<(Vec<u8>, String)> = encodings_with_groups().into_iter().collect();
    let initial: Vec<_> = VARIANTS
        .iter()
        .map(|&(iopl, cr4)| initial_fields(0, iopl, cr4))
        .collect();
    let outcomes = parallel_map_with(
        &encodings,
        || {
            VARIANTS
                .iter()
                .map(|&(iopl, cr4)| machine(0, iopl, cr4))
                .collect::<Vec<_>>()
        },
        |bases, (bytes, group)| {
            let expand = expanded(group);
            let mut digests = [0u64; 4];
            let mut lines = expand.then(String::new);
            for (variant, base) in bases.iter().enumerate() {
                let run = run_cloned(base, bytes, 1);
                let fields = outcome_fields(&run);
                digests[variant] = digest_fields(&fields);
                if let Some(lines) = &mut lines {
                    let _ = writeln!(
                        lines,
                        "{} v{variant} {:016x}{}",
                        hex(bytes),
                        digests[variant],
                        changed_fields(&initial[variant], &fields)
                    );
                }
            }
            EncodingOutcome { digests, lines }
        },
    );
    let count = encodings.len();
    let mut groups: BTreeMap<String, Vec<(Vec<u8>, EncodingOutcome)>> = BTreeMap::new();
    for ((bytes, group), outcome) in encodings.into_iter().zip(outcomes) {
        groups.entry(group).or_default().push((bytes, outcome));
    }
    Sweep {
        groups,
        encodings: count,
    }
}

/// The digest of a group over its encodings outside [`INTENTIONAL_ENCODINGS`], and their count.
fn group_digest(outcomes: &[(Vec<u8>, EncodingOutcome)]) -> (usize, u64) {
    let mut hash = cpl3_sweep::FNV_OFFSET;
    let mut count = 0;
    for (bytes, outcome) in outcomes {
        if intentional_encoding(bytes) {
            continue;
        }
        count += 1;
        for digest in outcome.digests {
            hash = fnv1a(hash, &digest.to_le_bytes());
        }
    }
    (count, hash)
}

fn intentional_count(sweep: &Sweep) -> usize {
    sweep
        .groups
        .values()
        .flatten()
        .filter(|(bytes, _)| intentional_encoding(bytes))
        .count()
}

const HEADER: &str = "\
# S7-32 ADR-0023 C1-0: the CPL0 baseline of the set over the reduced sweep (crates/od-isa/tests/cpl3_sweep).
# Written on G0 = ad625e3 (before any C1 change of src/) by `write_cpl0_baseline`; the machine of the sweep
# has on-demand mapping ON (part of the frozen `machine()`). Variants: CPL0 x IOPL {0, 3} x CR4 {0x6f8, 0x6f8 & !DE}.
# Line: group (map opcode [prefix set]) | encodings | FNV-1a 64 fold of the outcome digests; the encodings of
# INTENTIONAL_ENCODINGS (c1_cpl0_baseline.rs) are excluded from both and counted in `# intentional`.
";

#[test]
#[ignore = "writes the C1-0 baseline; run on G0 only with OD_WRITE_C1_BASELINE=1"]
fn write_cpl0_baseline() {
    if std::env::var("OD_WRITE_C1_BASELINE").as_deref() != Ok("1") {
        eprintln!("OD_WRITE_C1_BASELINE=1 not set: nothing written");
        return;
    }
    let sweep = sweep(|_| true);
    let mut baseline = String::from(HEADER);
    let mut outcomes = String::from(HEADER);
    let _ = writeln!(baseline, "# encodings {}", sweep.encodings);
    let _ = writeln!(baseline, "# intentional {}", intentional_count(&sweep));
    for (group, members) in &sweep.groups {
        let (count, digest) = group_digest(members);
        if count > 0 {
            let _ = writeln!(baseline, "{group}|{count}|{digest:016x}");
        }
        let _ = writeln!(outcomes, "## {group}");
        for (_, outcome) in members {
            outcomes.push_str(outcome.lines.as_deref().unwrap_or(""));
        }
    }
    std::fs::write(baseline_path(), baseline).expect("write the baseline");
    std::fs::create_dir_all(outcomes_path().parent().unwrap()).expect("outcome directory");
    std::fs::write(outcomes_path(), outcomes).expect("write the outcomes");
}

#[test]
fn cpl0_outcomes_match_the_recorded_baseline() {
    let recorded = std::fs::read_to_string(baseline_path()).expect("the C1-0 baseline");
    let mut expected: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut recorded_encodings = None;
    let mut recorded_intentional = None;
    for line in recorded.lines() {
        if let Some(count) = line.strip_prefix("# intentional ") {
            recorded_intentional = Some(count.parse::<usize>().unwrap());
            continue;
        }
        if let Some(count) = line.strip_prefix("# encodings ") {
            recorded_encodings = Some(count.parse::<usize>().unwrap());
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut fields = line.split('|');
        let group = fields.next().unwrap().to_string();
        let count = fields.next().unwrap().parse().unwrap();
        let digest = u64::from_str_radix(fields.next().unwrap(), 16).unwrap();
        expected.insert(group, (count, digest));
    }
    let dump = std::env::var("OD_DUMP_C1_GROUP").ok();
    let sweep = sweep(|group| dump.as_deref() == Some(group));
    if let Some(group) = &dump {
        match sweep.groups.get(group) {
            Some(members) => {
                for (_, outcome) in members {
                    print!("{}", outcome.lines.as_deref().unwrap_or(""));
                }
            }
            None => println!("no group {group:?} on the current code"),
        }
    }
    let mut differing = Vec::new();
    let mut skipped = Vec::new();
    let mut groups: Vec<&String> = expected.keys().chain(sweep.groups.keys()).collect();
    groups.sort();
    groups.dedup();
    for group in groups {
        // A group of intentional encodings only (e.g. a new LAR group) has nothing to compare.
        let now = sweep
            .groups
            .get(group)
            .map(|members| group_digest(members))
            .filter(|(count, _)| *count > 0);
        let then = expected.get(group).copied();
        if now == then {
            continue;
        }
        match intentional(group) {
            Some(reason) => skipped.push(format!("{group}: {then:?} -> {now:?} ({reason})")),
            None => differing.push(format!("{group}: recorded {then:?}, now {now:?}")),
        }
    }
    for line in &skipped {
        println!("intentional: {line}");
    }
    println!(
        "encodings: recorded {recorded_encodings:?}, now {}; intentional encodings: recorded \
         {recorded_intentional:?}, now {}; groups differing outside the lists: {}",
        sweep.encodings,
        intentional_count(&sweep),
        differing.len()
    );
    assert!(
        differing.is_empty(),
        "CPL0 outcomes differ from the C1-0 baseline in {} groups:\n{}",
        differing.len(),
        differing.join("\n")
    );
}
