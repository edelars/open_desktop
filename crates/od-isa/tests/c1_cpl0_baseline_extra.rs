//! S7-32, ADR-0023 slice C1 (review B7): an additional CPL0 baseline of the set, recorded on G0
//! (`ad625e3`, a `git archive` checkout with only the test files added), for the paths C1 changed
//! that the reduced sweep of `c1_cpl0_baseline` reaches weakly:
//!
//! - the FS/GS qword forms: prefix sets `64 48` and `65 48` over the maps `Primary` and `0F` (the
//!   60 FS/GS executors of `transfer` got their #AC check in C1);
//! - selectors inside the GDT: the groups `8E` (MOV Sreg), `CF` (IRET), `0F 00`, `0F 02`, `0F 03`,
//!   `0F A1`, `0F A9` of the reduced sweep on a machine whose registers and memory operands hold
//!   valid selectors (`0x2b` user data, `0x33` user code, `0x10` data, `0x53` the processor
//!   selector — in RCX of the IOPL 3 variants, review R5 — and `0x08` code) and whose stack holds a valid CPL0 `IRETQ` frame.
//!
//! Variants: CPL0 × IOPL {0, 3} × CR4 {`0x6f8`, without DE}. One digest per group (as
//! `c1_cpl0_baseline`). `write_cpl0_baseline_extra` (ignored; `OD_WRITE_C1_BASELINE=1`, output
//! directory `OD_C1_BASELINE_DIR` or `tests/data`) and `cpl0_outcomes_match_the_extra_baseline`.

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, CR4_DE, DATA, MAPS, RIP, STACK_PAGE, TAIL, decode, decoded_length, digest_fields,
    encodings_with_groups, fnv1a, group_name, machine, modrm_parts, outcome_fields,
    parallel_map_with, run_cloned,
};
use od_isa::{LongModeState, Register};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

const VARIANTS: [(u8, u64); 4] = [(0, CR4), (0, CR4 & !CR4_DE), (3, CR4), (3, CR4 & !CR4_DE)];

/// The encodings this baseline excludes on purpose (C1-5c): new decodes and the bare `VERW AX`,
/// which G0 refused at CPL0.
fn intentional(bytes: &[u8]) -> bool {
    let name = decode(bytes).map(|instruction| format!("{:?}", instruction.operation));
    name.as_deref()
        .is_ok_and(|name| name.starts_with("DescriptorQuery") || name == "VerwGdt")
}

/// The FS/GS qword encodings: maps `Primary` and `0F` × `64 48`, `65 48` × the ModRM parts.
fn fs_gs_encodings() -> BTreeMap<Vec<u8>, String> {
    let mut encodings = BTreeMap::new();
    for (map, map_bytes) in &MAPS[..2] {
        for opcode in 0..=255u8 {
            for prefixes in [&[0x64u8, 0x48][..], &[0x65, 0x48]] {
                for part in modrm_parts() {
                    let mut candidate = prefixes.to_vec();
                    candidate.extend_from_slice(map_bytes);
                    candidate.push(opcode);
                    candidate.extend_from_slice(&part);
                    candidate.extend_from_slice(&TAIL);
                    if let Some(length) = decoded_length(&candidate) {
                        candidate.truncate(length);
                        encodings
                            .entry(candidate)
                            .or_insert_with(|| group_name(map, opcode, prefixes));
                    }
                }
            }
        }
    }
    encodings
}

/// The groups of the reduced sweep that load or query selectors.
fn selector_encodings() -> BTreeMap<Vec<u8>, String> {
    encodings_with_groups()
        .into_iter()
        .filter(|(_, group)| {
            [
                "P 8E ", "P CF ", "0F 00 ", "0F 02 ", "0F 03 ", "0F A1 ", "0F A9 ",
            ]
            .iter()
            .any(|prefix| group.starts_with(prefix))
        })
        .map(|(bytes, group)| (bytes, format!("sel {group}")))
        .collect()
}

/// `machine(0, iopl, cr4)` with selectors: RAX 0x2b, RCX 0x33 (IOPL 3: 0x53), RDX 0x10, RBX 0x53, R8 0x08; the
/// word at [RDI] 0x2b, at [RDI + 0x10] 0x10; the RIP-relative window holds `2b 00` pairs; a CPL0
/// `IRETQ` frame (RIP + 0x100, CS 0x08, RFLAGS 0x202, RSP, SS 0x10) at RSP.
fn selector_machine(iopl: u8, cr4: u64) -> (LongModeState, cpl3_sweep::SweepMemory) {
    let (mut state, mut memory) = machine(0, iopl, cr4);
    for (register, value) in [
        (Register::Rax, 0x2b),
        // Review R5: the reduced sweep reaches RAX, RCX and RDI as register operands, so the processor
        // selector 0x53 (LSL of the TSS, LAR of a system descriptor) sits in RCX in the IOPL 3 variants.
        (Register::Rcx, if iopl == 3 { 0x53 } else { 0x33 }),
        (Register::Rdx, 0x10),
        (Register::Rbx, 0x53),
        (Register::R8, 0x08),
    ] {
        state.gpr[register as usize] = value;
    }
    let rdi = state.gpr[Register::Rdi as usize];
    memory.put_u16(rdi, 0x2b);
    memory.put_u16(rdi + 0x10, 0x10);
    for offset in (0x140..0x160u64).step_by(2) {
        memory.put_u16(cpl3_sweep::CODE_PAGE + offset, 0x2b);
    }
    let rsp = STACK_PAGE + 0xf00;
    state.gpr[Register::Rsp as usize] = rsp;
    for (index, value) in [RIP + 0x100, 0x08, 0x202, STACK_PAGE + 0xf80, 0x10]
        .into_iter()
        .enumerate()
    {
        memory.put_u64(rsp + 8 * index as u64, value);
    }
    let _ = DATA;
    (state, memory)
}

fn data_dir() -> PathBuf {
    std::env::var_os("OD_C1_BASELINE_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data"),
        PathBuf::from,
    )
}

/// Group → (encodings outside `intentional`, digest).
fn sweep() -> BTreeMap<String, (usize, u64)> {
    let mut all: Vec<(Vec<u8>, String, bool)> = fs_gs_encodings()
        .into_iter()
        .map(|(bytes, group)| (bytes, group, false))
        .collect();
    all.extend(
        selector_encodings()
            .into_iter()
            .map(|(bytes, group)| (bytes, group, true)),
    );
    let digests = parallel_map_with(
        &all,
        || {
            let plain: Vec<_> = VARIANTS.iter().map(|&(i, c)| machine(0, i, c)).collect();
            let selectors: Vec<_> = VARIANTS
                .iter()
                .map(|&(i, c)| selector_machine(i, c))
                .collect();
            (plain, selectors)
        },
        |(plain, selectors), (bytes, _, selector)| {
            let bases = if *selector { &*selectors } else { &*plain };
            bases
                .iter()
                .map(|base| digest_fields(&outcome_fields(&run_cloned(base, bytes, 1))))
                .collect::<Vec<u64>>()
        },
    );
    let mut groups: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for ((bytes, group, _), digests) in all.iter().zip(digests) {
        if intentional(bytes) {
            continue;
        }
        let entry = groups
            .entry(group.clone())
            .or_insert((0, cpl3_sweep::FNV_OFFSET));
        entry.0 += 1;
        for digest in digests {
            entry.1 = fnv1a(entry.1, &digest.to_le_bytes());
        }
    }
    groups
}

#[test]
#[ignore = "writes the extra baseline; run on the G0 checkout with OD_WRITE_C1_BASELINE=1"]
fn write_cpl0_baseline_extra() {
    if std::env::var("OD_WRITE_C1_BASELINE").as_deref() != Ok("1") {
        return;
    }
    let mut out = String::from(
        "# S7-32 ADR-0023 C1 (review B7): the extra CPL0 baseline (c1_cpl0_baseline_extra.rs), written on G0 = ad625e3\n\
         # (git archive checkout + the test files). FS/GS qword forms and selector inputs; line: group|encodings|digest.\n",
    );
    for (group, (count, digest)) in sweep() {
        let _ = writeln!(out, "{group}|{count}|{digest:016x}");
    }
    std::fs::write(data_dir().join("c1_cpl0_baseline_extra.txt"), out).unwrap();
}

#[test]
fn cpl0_outcomes_match_the_extra_baseline() {
    let recorded = std::fs::read_to_string(data_dir().join("c1_cpl0_baseline_extra.txt")).unwrap();
    let mut expected = BTreeMap::new();
    for line in recorded.lines().filter(|line| !line.starts_with('#')) {
        let mut fields = line.split('|');
        let group = fields.next().unwrap().to_string();
        let count: usize = fields.next().unwrap().parse().unwrap();
        let digest = u64::from_str_radix(fields.next().unwrap(), 16).unwrap();
        expected.insert(group, (count, digest));
    }
    let now = sweep();
    let mut differing = Vec::new();
    for group in expected
        .keys()
        .chain(now.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if expected.get(group) != now.get(group) {
            differing.push(format!(
                "{group}: recorded {:?}, now {:?}",
                expected.get(group),
                now.get(group)
            ));
        }
    }
    println!("groups: recorded {}, now {}", expected.len(), now.len());
    assert!(
        differing.is_empty(),
        "{} groups differ:\n{}",
        differing.len(),
        differing.join("\n")
    );
}
