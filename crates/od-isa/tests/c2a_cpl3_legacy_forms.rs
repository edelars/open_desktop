//! ADR-0023 slice C2a-1 (plan `work/s7-32/c1/plan-c1-c2a.md`, C2a-1): the corpus and the golden of
//! the legacy CPL3 paths, recorded on the code of slice C1 before the allow-list and the parallel
//! paths were removed (C2a-4).
//!
//! - `tests/data/c2a_cpl3_legacy_corpus.txt`: the encodings (the reduced sweep of C1-0, the byte
//!   literals of the CPL3 tests, the executable sections of the Linux user ELFs of S6-21/S6-22),
//!   de-duplicated by `tests/data/c2a_cpl3_legacy_corpus_gen.py` from the output of
//!   [`classify_legacy_candidates`] (the cut into instructions, the signature and the flag "the old
//!   CPL3 entry accepts" are computed here, on the old code).
//! - `tests/data/c2a_cpl3_legacy_golden.txt` (review F5, decision of the coordinator): for each
//!   corpus encoding (sources (b) and (c), in corpus order) the FNV-1a 64 fold of its twelve outcome
//!   digests (the `outcome_digest` of C1-0) — FP member {0, 1} × the inputs A, B, C, D, E, E' of
//!   [`cpl3_sweep::InputVariant`], `Unsupported` of the old entry as [`NONE_DIGEST`] — one instruction
//!   through the CPL3 entry with the set of the Linux recipe; and for the source (a), the deterministic
//!   reduced sweep of C1-0 (`cpl3_sweep::encodings_with_groups`), the fold of those folds per group.
//!   Recorded on the code of slice C1 (`bc59282`) in a clean `git archive` copy with the test files.
//! - The unreduced golden of the plan format (`hex fp dA dB dC dD dE dE'`, all three sources) stays in
//!   `work/s7-32/c2a/corpus/`; `OD_C2A_CORPUS`/`OD_C2A_GOLDEN` select it for the per-input comparison.
//!
//! On the new code [`legacy_forms_keep_their_recorded_outcomes`] checks every fold (decision 2 of
//! ADR-0023: the rows of the header `# decision-2` only) and
//! [`legacy_golden_covers_every_form_the_old_paths_named`] the operations and the form signatures
//! the golden covers.

mod cpl3_sweep;

use cpl3_sweep::*;
use od_isa::Cpl0ProfileSetExit;
use od_isa::execute::RunExit;
use std::collections::{BTreeMap, BTreeSet};

const CORPUS_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c2a_cpl3_legacy_corpus.txt"
);
const SWEEP_GOLDEN_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c2a_cpl3_legacy_sweep_golden.txt"
);
const GOLDEN_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c2a_cpl3_legacy_golden.txt"
);

/// The working directory of the generator (not in Git).
const WORK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../work/s7-32/c2a/corpus");

/// One instruction of `bytes` through the CPL3 entry with the Linux set (`fp_member`) on `input`.
pub fn legacy_run(bytes: &[u8], fp_member: bool, input: InputVariant) -> Run {
    let (state, memory) = input_machine(3, LINUX_CR4, input);
    run_on_with(state, memory, Cells::new(), bytes, 1, |cells| {
        linux_set(cells, fp_member)
    })
}

/// Whether the run ended with `Unsupported` (an admission error panics: a defect of the fixture).
pub fn unsupported(run: &Run) -> bool {
    match &run.result {
        Ok(result) => matches!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { .. })
        ),
        Err(error) => panic!("the fixture is not admitted: {error:?}"),
    }
}

/// Plan C2a-1 item 3: the outcome digest of one input; `None` for `Unsupported`.
pub fn legacy_digest(bytes: &[u8], fp_member: bool, input: InputVariant) -> Option<u64> {
    let run = legacy_run(bytes, fp_member, input);
    (!unsupported(&run)).then(|| outcome_digest(&run))
}

/// The operation name of an encoding: its `Debug` text up to the first non-identifier character.
pub fn operation_name(bytes: &[u8]) -> String {
    let instruction = decode(bytes).expect("a corpus encoding decodes");
    format!("{:?}", instruction.operation)
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn data_lines(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// The encodings of the corpus.
pub fn corpus() -> Vec<Vec<u8>> {
    data_lines(CORPUS_FILE)
        .iter()
        .map(|line| unhex(line.split_whitespace().next().unwrap()))
        .collect()
}

/// One row of the unreduced golden (plan format): the encoding, the FP member and six digests.
pub struct GoldenRow {
    pub bytes: Vec<u8>,
    pub fp_member: bool,
    pub digests: [Option<u64>; 6],
}

/// The rows of the unreduced golden of `OD_C2A_GOLDEN` (plan format).
pub fn unreduced_golden(path: &str) -> Vec<GoldenRow> {
    data_lines(path)
        .iter()
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 8, "{line}");
            GoldenRow {
                bytes: unhex(fields[0]),
                fp_member: fields[1] == "1",
                digests: core::array::from_fn(|index| {
                    let field = fields[2 + index];
                    (field != "-").then(|| u64::from_str_radix(field, 16).expect("digest"))
                }),
            }
        })
        .collect()
}

/// The digests of one encoding and FP member on the six inputs.
fn digests(bytes: &[u8], fp_member: bool) -> [Option<u64>; 6] {
    INPUTS.map(|input| legacy_digest(bytes, fp_member, input))
}

/// The digest that stands for an `Unsupported` of the old entry in a fold ("NONENONE").
pub const NONE_DIGEST: u64 = 0x4e4f_4e45_4e4f_4e45;

/// The twelve digests of an encoding: FP member 0 then 1, the inputs A..E' each.
fn twelve(bytes: &[u8]) -> [Option<u64>; 12] {
    let (without, with) = (digests(bytes, false), digests(bytes, true));
    core::array::from_fn(|index| {
        if index < 6 {
            without[index]
        } else {
            with[index - 6]
        }
    })
}

/// The FNV-1a 64 fold of twelve digests ([`NONE_DIGEST`] for `None`).
fn fold(digests: &[Option<u64>; 12]) -> u64 {
    digests.iter().fold(FNV_OFFSET, |hash, digest| {
        fnv1a(hash, &digest.unwrap_or(NONE_DIGEST).to_le_bytes())
    })
}

/// The recorded inputs of twelve digests: bit `i` set for `Some`.
fn mask(digests: &[Option<u64>; 12]) -> u16 {
    digests.iter().enumerate().fold(0, |mask, (index, digest)| {
        mask | u16::from(digest.is_some()) << index
    })
}

/// The new digests with the inputs the old entry did not record (`Unsupported`, plan C2a-1: "входы `-`
/// не сравниваются") set to `None`.
fn masked(mut digests: [Option<u64>; 12], recorded: u16) -> [Option<u64>; 12] {
    for (index, digest) in digests.iter_mut().enumerate() {
        if recorded & (1 << index) == 0 {
            *digest = None;
        }
    }
    digests
}

/// Run-length text of masks: `mmm*count` items joined by `,`.
fn rle(masks: &[u16]) -> String {
    let mut items: Vec<(u16, usize)> = Vec::new();
    for &mask in masks {
        match items.last_mut() {
            Some((last, count)) if *last == mask => *count += 1,
            _ => items.push((mask, 1)),
        }
    }
    items
        .iter()
        .map(|(mask, count)| format!("{mask:03x}*{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn unrle(text: &str) -> Vec<u16> {
    text.split(',')
        .flat_map(|item| {
            let (mask, count) = item.split_once('*').expect("mask*count");
            std::iter::repeat_n(
                u16::from_str_radix(mask, 16).unwrap(),
                count.parse().unwrap(),
            )
        })
        .collect()
}

/// The sweep of C1-0 by group, the encodings of a group in byte order.
fn sweep_groups() -> BTreeMap<String, Vec<Vec<u8>>> {
    let mut groups: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    for (bytes, group) in encodings_with_groups() {
        groups.entry(group).or_default().push(bytes);
    }
    groups
}

/// The compact golden: the corpus folds with their masks of recorded inputs, the group folds with the
/// masks of their encodings, the decision-2 rows (hex, input index 0..12, old digest).
struct Compact {
    corpus_fnv: u64,
    encodings: Vec<(u64, u16)>,
    groups: BTreeMap<String, (u64, Vec<u16>)>,
    decision_2: Vec<(String, usize, u64)>,
}

fn read_compact() -> Compact {
    let text = std::fs::read_to_string(GOLDEN_FILE).unwrap()
        + &std::fs::read_to_string(SWEEP_GOLDEN_FILE).unwrap();
    let mut compact = Compact {
        corpus_fnv: 0,
        encodings: Vec::new(),
        groups: BTreeMap::new(),
        decision_2: Vec::new(),
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("# corpus fnv1a64 ") {
            compact.corpus_fnv = u64::from_str_radix(value, 16).unwrap();
        } else if let Some(entry) = line.strip_prefix("# decision-2 ") {
            let fields: Vec<&str> = entry.split_whitespace().collect();
            if fields != ["none"] {
                compact.decision_2.push((
                    fields[0].to_owned(),
                    fields[1].parse().unwrap(),
                    u64::from_str_radix(fields[2], 16).unwrap(),
                ));
            }
        } else if line.starts_with('#') || line.is_empty() {
        } else if let Some((group, rest)) = line.split_once('|') {
            let (value, masks) = rest.split_once('|').expect("group|fold|masks");
            compact.groups.insert(
                group.to_owned(),
                (u64::from_str_radix(value, 16).unwrap(), unrle(masks)),
            );
        } else {
            let (value, recorded) = line.split_at(16);
            compact.encodings.push((
                u64::from_str_radix(value, 16).unwrap(),
                u16::from_str_radix(recorded, 16).unwrap(),
            ));
        }
    }
    compact
}

/// The operations the step ran on `state.sse` for a set without the FP member (the forms of
/// `step_cpl3_xmm`, `sse_cpl0_cpl3_handler_form` on the code of slice C1): decision 2 of ADR-0023
/// makes them `Unsupported` without the member; the writer records, for the rows without the member,
/// the inputs where such a form retired (`# decision-2`).
const SSE_WITHOUT_THE_MEMBER: &[&str] = &[
    "MoveIntegerToXmm",
    "MovqFromXmm",
    "PshuflwRegister",
    "PinsrwMemory",
    "MovlpdLoadMemory",
    "MovhpdLoadMemory",
    "MovhpsLoadMemory",
    "PminubMemory",
    "PcmpeqbMemory",
];

/// Plan C2a-1 item 1 (a): the reduced sweep of C1-0, one encoding per line (generator input).
#[test]
#[ignore = "generator: OD_C2A_DUMP_SWEEP=1 writes work/s7-32/c2a/corpus/sweep.txt"]
fn dump_sweep() {
    if std::env::var_os("OD_C2A_DUMP_SWEEP").is_none() {
        return;
    }
    let text: String = encodings().iter().map(|bytes| hex(bytes) + "\n").collect();
    std::fs::create_dir_all(WORK).unwrap();
    std::fs::write(format!("{WORK}/sweep.txt"), text).unwrap();
}

/// Cut a byte stream into instructions linearly: a decoded instruction, else the measured length,
/// else one byte.
fn cut(stream: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < stream.len() {
        let window = &stream[at..stream.len().min(at + 15)];
        let step = match decode(window) {
            Ok(instruction) => {
                let length = usize::from(instruction.length);
                out.push(window[..length].to_vec());
                length
            }
            Err(_) => od_isa::length::long64_length(window).unwrap_or(1),
        };
        at += step.max(1);
    }
    out
}

/// Plan C2a-1 item 1: the cut of the candidates into instructions and the flag "the old CPL3 entry
/// accepts" (some input of A-D with or without the FP member is not `Unsupported`), with the form
/// signature and the sources (`a`, `b`, `c`).
#[test]
#[ignore = "generator: OD_C2A_CLASSIFY=1 reads candidates.txt and writes classified.txt in work/s7-32/c2a/corpus"]
fn classify_legacy_candidates() {
    if std::env::var_os("OD_C2A_CLASSIFY").is_none() {
        return;
    }
    let text = std::fs::read_to_string(format!("{WORK}/candidates.txt")).unwrap();
    let mut sources: BTreeMap<Vec<u8>, BTreeSet<char>> = BTreeMap::new();
    for line in text
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
    {
        let mut fields = line.split_whitespace();
        let (kind, source, bytes) = (
            fields.next().unwrap(),
            fields.next().unwrap(),
            unhex(fields.next().unwrap()),
        );
        let tag = source.chars().next().unwrap();
        let instructions = if kind == "insn" {
            decode(&bytes)
                .ok()
                .map(|instruction| vec![bytes[..usize::from(instruction.length)].to_vec()])
                .unwrap_or_default()
        } else {
            cut(&bytes)
        };
        for instruction in instructions {
            sources.entry(instruction).or_default().insert(tag);
        }
    }
    let items: Vec<(Vec<u8>, BTreeSet<char>)> = sources.into_iter().collect();
    let rows = parallel_map_with(
        &items,
        || (),
        |_, (bytes, tags)| {
            let accepted = [false, true].iter().any(|&fp_member| {
                INPUTS[..4]
                    .iter()
                    .any(|&input| !unsupported(&legacy_run(bytes, fp_member, input)))
            });
            let signature = form_signature(bytes).unwrap_or_else(|| "unmeasured".into());
            let tags: Vec<String> = tags.iter().map(char::to_string).collect();
            format!(
                "{} {} {} {}\n",
                hex(bytes),
                u8::from(accepted),
                signature,
                tags.join(",")
            )
        },
    );
    std::fs::write(format!("{WORK}/classified.txt"), rows.concat()).unwrap();
}

/// Plan C2a-1 item 3 (review F5): the compact golden of the old paths, written on the code of slice
/// C1 (`OD_WRITE_C2A_GOLDEN=<commit>`).
#[test]
#[ignore = "generator: OD_WRITE_C2A_GOLDEN=<commit> writes tests/data/c2a_cpl3_legacy_golden.txt"]
fn write_legacy_golden() {
    let Ok(commit) = std::env::var("OD_WRITE_C2A_GOLDEN") else {
        return;
    };
    let corpus_encodings = corpus();
    let groups: Vec<(String, Vec<Vec<u8>>)> = sweep_groups().into_iter().collect();
    // (fold, recorded, decision-2 rows)
    let one = |bytes: &Vec<u8>| {
        let digests = twelve(bytes);
        let name = operation_name(bytes);
        let decision_2: Vec<(String, usize, u64)> =
            if SSE_WITHOUT_THE_MEMBER.contains(&name.as_str()) {
                INPUTS
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &input)| {
                        let run = legacy_run(bytes, false, input);
                        let retired = run
                            .result
                            .as_ref()
                            .is_ok_and(|result| result.instructions_executed > 0);
                        retired.then(|| (hex(bytes), index, digests[index].unwrap()))
                    })
                    .collect()
            } else {
                Vec::new()
            };
        (fold(&digests), mask(&digests), decision_2)
    };
    let rows = parallel_map_with(&corpus_encodings, || (), |_, bytes| one(bytes));
    let group_rows = parallel_map_with(
        &groups,
        || (),
        |_, (group, members)| {
            let mut hash = FNV_OFFSET;
            let mut decision_2 = Vec::new();
            let mut masks = Vec::new();
            for bytes in members {
                let (value, recorded, rows) = one(bytes);
                hash = fnv1a(hash, &value.to_le_bytes());
                masks.push(recorded);
                decision_2.extend(rows);
            }
            (group.clone(), hash, masks, decision_2)
        },
    );
    let corpus_text = std::fs::read(CORPUS_FILE).unwrap();
    let mut decision_2: Vec<String> = rows
        .iter()
        .flat_map(|(_, _, rows)| rows.clone())
        .chain(group_rows.iter().flat_map(|(_, _, _, rows)| rows.clone()))
        .map(|(hex, index, digest)| format!("# decision-2 {hex} {index} {digest:016x}\n"))
        .collect();
    decision_2.sort();
    decision_2.dedup();
    if decision_2.is_empty() {
        decision_2.push("# decision-2 none\n".into());
    }
    let mut out = format!(
        "# ADR-0023 slice C2a-1: the golden of the legacy CPL3 paths, recorded on {commit} (slice C1, a clean\n\
         # `git archive` copy with the test files) before the removal of the allow-list (C2a-4).\n\
         # Per corpus encoding (corpus order): the FNV-1a 64 fold of the twelve outcome digests (FP member\n\
         # 0/1 x inputs A B C D E E'; `outcome_digest` of C1-0; Unsupported = NONE_DIGEST) and the 12-bit mask of\n\
         # the recorded inputs (hex). The sweep of C1-0: `c2a_cpl3_legacy_sweep_golden.txt`.\n\
         # corpus fnv1a64 {:016x}\n{}",
        fnv1a(FNV_OFFSET, &corpus_text),
        std::env::var("OD_C2A_FULL_GOLDEN")
            .map(|full| format!("# unreduced golden (work/, not in Git): {full}\n"))
            .unwrap_or_default()
    );
    for line in decision_2 {
        out.push_str(&line);
    }
    for (value, recorded, _) in &rows {
        out.push_str(&format!("{value:016x}{recorded:03x}\n"));
    }
    std::fs::write(GOLDEN_FILE, out).unwrap();
    let mut sweep = format!(
        "# ADR-0023 slice C2a-1: the golden of the legacy CPL3 paths over the reduced sweep of C1-0 (source (a)),\n\
         # recorded on {commit} (slice C1, a clean `git archive` copy with the test files). Line: group | the\n\
         # FNV-1a 64 fold of the folds of its encodings in byte order (the fold of `c2a_cpl3_legacy_golden.txt`) |\n\
         # the masks of their recorded inputs, run-length coded (`mask*count`).\n"
    );
    for (group, value, masks, _) in &group_rows {
        sweep.push_str(&format!("{group}|{value:016x}|{}\n", rle(masks)));
    }
    std::fs::write(SWEEP_GOLDEN_FILE, sweep).unwrap();
}

/// The fold of an encoding on the new code over the inputs the old entry recorded (`recorded`), with
/// the decision-2 inputs of the golden: the new outcome must be `Unsupported` there and the old digest
/// stands in the fold.
fn new_fold(
    bytes: &[u8],
    recorded: u16,
    decision_2: &[(String, usize, u64)],
    changed: &mut Vec<String>,
) -> u64 {
    let mut digests = masked(twelve(bytes), recorded);
    let text = hex(bytes);
    for (encoding, index, old) in decision_2 {
        if *encoding == text {
            if digests[*index].is_none() {
                changed.push(format!("{text} input {index}"));
                digests[*index] = Some(*old);
            }
        }
    }
    fold(&digests)
}

#[test]
fn legacy_forms_keep_their_recorded_outcomes() {
    let golden = read_compact();
    let corpus_text = std::fs::read(CORPUS_FILE).unwrap();
    assert_eq!(
        golden.corpus_fnv,
        fnv1a(FNV_OFFSET, &corpus_text),
        "the golden was recorded for another corpus"
    );
    let encodings = corpus();
    assert_eq!(encodings.len(), golden.encodings.len());
    let groups: Vec<(String, Vec<Vec<u8>>)> = sweep_groups().into_iter().collect();
    assert_eq!(groups.len(), golden.groups.len(), "the groups of the sweep");
    let items: Vec<(&Vec<u8>, u16)> = encodings
        .iter()
        .zip(&golden.encodings)
        .map(|(bytes, (_, recorded))| (bytes, *recorded))
        .collect();
    let corpus_results = parallel_map_with(
        &items,
        || (),
        |_, (bytes, recorded)| {
            let mut changed = Vec::new();
            (
                new_fold(bytes, *recorded, &golden.decision_2, &mut changed),
                changed,
            )
        },
    );
    let group_results = parallel_map_with(
        &groups,
        || (),
        |_, (group, members)| {
            let mut changed = Vec::new();
            let mut hash = FNV_OFFSET;
            let masks = golden
                .groups
                .get(group)
                .map(|(_, masks)| masks.clone())
                .unwrap_or_default();
            assert_eq!(
                masks.len(),
                members.len(),
                "the encodings of the group {group}"
            );
            for (bytes, recorded) in members.iter().zip(masks) {
                let value = new_fold(bytes, recorded, &golden.decision_2, &mut changed);
                hash = fnv1a(hash, &value.to_le_bytes());
            }
            (group.clone(), hash, changed)
        },
    );
    let mut differences = Vec::new();
    let mut decision_2 = Vec::new();
    for ((bytes, (now, changed)), (then, _)) in
        encodings.iter().zip(&corpus_results).zip(&golden.encodings)
    {
        decision_2.extend(changed.clone());
        if now != then {
            differences.push(format!("corpus {} {}", hex(bytes), operation_name(bytes)));
        }
    }
    for (group, now, changed) in &group_results {
        decision_2.extend(changed.clone());
        if golden.groups.get(group).map(|(value, _)| value) != Some(now) {
            differences.push(format!("sweep group {group}"));
        }
    }
    println!(
        "decision 2 (SSE without the FP member, now Unsupported): {} {decision_2:?}",
        decision_2.len()
    );
    assert!(
        differences.is_empty(),
        "{} recorded folds changed (OD_C2A_CORPUS/OD_C2A_GOLDEN = the unreduced golden of work/ gives the input):\n{}",
        differences.len(),
        differences
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The per-input comparison with the unreduced golden of the plan format (`OD_C2A_GOLDEN`, with its
/// corpus in `OD_C2A_CORPUS`); a no-op without them. Decision 2: an input whose old outcome was a
/// retirement of a form of [`SSE_WITHOUT_THE_MEMBER`] without the member may become `Unsupported`.
#[test]
fn legacy_forms_keep_their_recorded_outcomes_in_the_unreduced_golden() {
    let Ok(path) = std::env::var("OD_C2A_GOLDEN") else {
        return;
    };
    let rows = unreduced_golden(&path);
    let results = parallel_map_with(
        &rows,
        || (),
        |_, row| {
            let now = digests(&row.bytes, row.fp_member);
            let mut differences = Vec::new();
            for (index, (old, new)) in row.digests.iter().zip(now).enumerate() {
                let Some(old) = old else { continue };
                if Some(*old) == new {
                    continue;
                }
                let name = operation_name(&row.bytes);
                let retired_before = !row.fp_member
                    && new.is_none()
                    && SSE_WITHOUT_THE_MEMBER.contains(&name.as_str());
                differences.push(format!(
                    "{} fp {} {name} input {:?}: {old:016x} -> {new:x?}{}",
                    hex(&row.bytes),
                    u8::from(row.fp_member),
                    INPUTS[index],
                    if retired_before {
                        " (decision 2 candidate)"
                    } else {
                        ""
                    }
                ));
            }
            differences
        },
    );
    let differences: Vec<String> = results.into_iter().flatten().collect();
    assert!(
        differences.is_empty(),
        "{} changed:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

/// Plan C2a-1 item 3: every operation named by the old paths (listed before their removal by
/// `work/s7-32/c2a/corpus/old-path-operations.txt`: `Operation::` of `cpl3_rep_string_shape`,
/// `cpl3_allows` and its helpers, `cpl3_scalar_fp_form`, `sse_cpl0_cpl3_handler_form`,
/// `cpl3_sse_set_form` and the CPL3 branch of the step before the FP member) has a golden row with a
/// recorded input.
const OLD_PATH_OPERATIONS: &[&str] = &[
    "AddFsLoad",
    "AddsdRegister",
    "Arithmetic",
    "Bit",
    "BitScan",
    "Branch",
    "Bswap",
    "Call",
    "CarryFlag",
    "CetNop",
    "CmpFsLoad",
    "CmpFsMem",
    "CmpFsMemImm",
    "ComissUcomissRegister",
    "ConditionalMove",
    "Cpuid",
    "Cvtsi2sdR64Register",
    "Cvttsd2siR64Register",
    "Divide",
    "Endbr64Nop",
    "Extend",
    "Imul",
    "ImulImmediate",
    "IndirectCall",
    "IndirectJump",
    "Jump",
    "LdmxcsrMemory",
    "Lea",
    "Leave",
    "LockAddMem",
    "LockAddMemReg",
    "LockBitMemImm",
    "LockCmpxchgMem",
    "LockOrMem",
    "LockSubDwordImm8",
    "LockXaddMem",
    "Logic",
    "LogicFsLoad",
    "Mov",
    "MovFsLoad",
    "MovFsLoadD",
    "MovFsLoadQ",
    "MovFsMemImm",
    "MovFsStore",
    "MovImmediate",
    "MovapsLoadMemory",
    "MovapsRegister",
    "MovapsStoreMemory",
    "MovdqaLoadMemory",
    "MovdqaRegister",
    "MovdquLoadMemory",
    "MoveIntegerToXmm",
    "MovhpdLoadMemory",
    "MovhpsLoadMemory",
    "MovlpdLoadMemory",
    "MovqFromXmm",
    "MovsdLoadMemory",
    "MovsdStoreMemory",
    "MovupdLoadMemory",
    "MovupdStoreMemory",
    "MovupsLoadMemory",
    "MovupsStoreMemory",
    "Mul",
    "MulsdMemory",
    "MulsdRegister",
    "Nop",
    "NopModrm",
    "PackuswbRegister",
    "PaddbRegister",
    "PadddRegister",
    "PaddqRegister",
    "PandRegister",
    "PandnRegister",
    "PcmpeqbMemory",
    "PcmpeqbRegister",
    "PcmpeqdRegister",
    "PcmpgtbRegister",
    "PinsrwMemory",
    "PminubMemory",
    "PminubRegister",
    "PmovmskbXmmToGpr",
    "PorRegister",
    "PshufdRegister",
    "PshuflwRegister",
    "PslldqImmediateRegister",
    "PsrldqImmediateRegister",
    "PsrlwImmediateRegister",
    "PsubbRegister",
    "PunpckhwdRegister",
    "PunpcklbwRegister",
    "PunpckldqRegister",
    "PunpcklqdqRegister",
    "PunpcklwdRegister",
    "PushImmediate",
    "PushMemory",
    "PxorRegister",
    "RdsspqRax",
    "Rdtsc",
    "ReadMsr",
    "RepMovs",
    "RepStos",
    "Return",
    "SetCondition",
    "Shift",
    "SignExtendAccumulator",
    "SoftwareInterrupt",
    "Sse128MemoryMove",
    "Sse2Int",
    "Sse2Maskmov",
    "Sse3Permute",
    "Sse41",
    "Sse41Fp",
    "SseCvtFromInt",
    "SseCvtToInt",
    "SseFpArith",
    "SseFpMisc",
    "SseLogicPermute",
    "SseMovmsk",
    "Ssse3Int",
    "StackRegister",
    "SubFsMem",
    "SubsdRegister",
    "Syscall",
    "Sysret",
    "TestFsMemByte",
    "WriteMsr",
    "Xchg",
    "XchgMemory",
    "XorpsRegister",
];

/// The operations of the old paths that the Linux set of the golden never runs, with the reason;
/// the differential test (`c2a_cpl3_neutral_differential`, sweep set with the Windows CPUID member)
/// covers them CPL0 against CPL3.
const NOT_RUN_BY_THE_LINUX_SET: &[(&str, &str)] = &[(
    "LdmxcsrMemory",
    "stage4a-apic declares no SSE: step_cpl0_fp refuses LDMXCSR at CPL0 and CPL3 (Unsupported)",
)];

/// The five forms of `cpl3_rep_string_shape`.
const REP_STRING_FORMS: [&[u8]; 5] = [
    &[0xf3, 0xa4],
    &[0xf3, 0xa5],
    &[0xf3, 0xaa],
    &[0xf3, 0xab],
    &[0xf3, 0x48, 0xab],
];

/// The form signatures of the unreduced golden of the plan rules (`work/`, 109 449 rows): the
/// recorded encodings of the golden in Git (at least one recorded input) cover at least as many
/// (review F5, G1).
const UNREDUCED_GOLDEN_SIGNATURES: usize = 52_996;

#[test]
fn legacy_golden_covers_every_form_the_old_paths_named() {
    let golden = read_compact();
    let encodings = corpus();
    assert_eq!(encodings.len(), golden.encodings.len());
    // An encoding counts only with a recorded input (mask ≠ 0): an encoding refused on all twelve
    // inputs has no outcome in the golden (review G1).
    let mut recorded_encodings: Vec<&Vec<u8>> = encodings
        .iter()
        .zip(&golden.encodings)
        .filter(|(_, (_, recorded))| *recorded != 0)
        .map(|(bytes, _)| bytes)
        .collect();
    // The sweep of C1-0 is in the golden by its group folds, with a mask per encoding.
    let groups = sweep_groups();
    for (group, members) in &groups {
        let masks = golden
            .groups
            .get(group)
            .map(|(_, masks)| masks.as_slice())
            .unwrap_or_default();
        assert_eq!(
            masks.len(),
            members.len(),
            "the encodings of the group {group}"
        );
        recorded_encodings.extend(
            members
                .iter()
                .zip(masks)
                .filter(|(_, recorded)| **recorded != 0)
                .map(|(bytes, _)| bytes),
        );
    }
    let mut named = BTreeSet::new();
    let mut forms = BTreeSet::new();
    let mut signatures = BTreeSet::new();
    for bytes in recorded_encodings {
        named.insert(operation_name(bytes));
        signatures.insert(form_signature(bytes));
        forms.insert(bytes.clone());
    }
    let missing: Vec<&&str> = OLD_PATH_OPERATIONS
        .iter()
        .filter(|name| !named.contains(**name))
        .filter(|name| {
            NOT_RUN_BY_THE_LINUX_SET
                .iter()
                .all(|(excluded, _)| excluded != *name)
        })
        .collect();
    assert!(
        missing.is_empty(),
        "operations without a golden form: {missing:?}"
    );
    for (excluded, reason) in NOT_RUN_BY_THE_LINUX_SET {
        assert!(
            !named.contains(*excluded),
            "{excluded} has a recorded golden form, the exclusion ({reason}) is stale"
        );
    }
    for form in REP_STRING_FORMS {
        assert!(forms.contains(form), "no golden form {}", hex(form));
    }
    assert!(
        signatures.len() >= UNREDUCED_GOLDEN_SIGNATURES,
        "{} form signatures, the unreduced golden has {UNREDUCED_GOLDEN_SIGNATURES}",
        signatures.len()
    );
}
