//! S7-32, ADR-0023 slice C1, plan step C1-7: the CPL3 classification and the privilege layer against
//! an independent model written from the Intel SDM 325462-093US opcode maps and instruction pages,
//! without `cpl3_class` and without reading `long_decode`:
//!
//! - Vol. 3A §6.9 and §2.8 Table 2-3: the privileged instructions (LGDT, LIDT, LLDT, LTR, LMSW, CLTS,
//!   MOV CRn, MOV DRn, INVD, WBINVD, INVLPG, HLT, RDMSR, WRMSR); Vol. 2B "SWAPGS", "SYSRET",
//!   "SYSEXIT" (#GP(0) if CPL ≠ 0);
//! - Vol. 2B "RDTSC" (CR4.TSD), "RDPMC" (CR4.PCE), "SGDT", "SIDT", "SLDT", "STR", "SMSW" (CR4.UMIP);
//! - Vol. 1 §20.5, Vol. 2A "CLI", "IN", "INS", Vol. 2B "STI", "OUT", "OUTS", "POPF": the IOPL rules;
//! - Vol. 2A "INT n/INTO/INT3/INT1", "IRET", "LAR", "LSL", "VERR/VERW", "MOV" (to Sreg), "POP" (FS/GS),
//!   "SYSCALL": executors with their own CPL rules;
//! - far returns (`CA`, `CB`) and SYSENTER: `NotAtCpl3` (ADR-0023 decision 1; plan question V12).
//!
//! `classification_of_every_decoded_encoding_matches_the_sdm_model` covers every encoding the decoder
//! accepts in the full classification sweep; `cpl3_outcomes_match_the_sdm_model` runs the reduced
//! sweep at CPL3 for the encodings the model does not call `Neutral` and checks the decision of the
//! layer. CPL0 outcomes are the C1-0 baseline (`c1_cpl0_baseline`), not repeated here.

mod cpl3_sweep;

use cpl3_sweep::{
    ALLOWED_PORTS, CR4, Cells, GP0, MAPS, RIP, Run, SweepMemory, UD, encodings_with_groups,
    machine_cached, pattern, run_exit, run_on,
};
use od_isa::execute::RunExit;
use od_isa::{Cpl3Class, Cr4Gate, IoplKind, LongModeState, Register, cpl3_class};

/// The class of the model (the same names as ADR-0023 decision 1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Model {
    Neutral,
    Privileged,
    Tsd,
    Pce,
    Umip,
    InterruptFlag,
    PortIo,
    Popf,
    CplSpecific,
    NotAtCpl3,
}

impl Model {
    fn class(self) -> Cpl3Class {
        match self {
            Model::Neutral => Cpl3Class::Neutral,
            Model::Privileged => Cpl3Class::Privileged,
            Model::Tsd => Cpl3Class::Cr4Gated(Cr4Gate::Tsd),
            Model::Pce => Cpl3Class::Cr4Gated(Cr4Gate::Pce),
            Model::Umip => Cpl3Class::Cr4Gated(Cr4Gate::Umip),
            Model::InterruptFlag => Cpl3Class::IoplSensitive(IoplKind::InterruptFlag),
            Model::PortIo => Cpl3Class::IoplSensitive(IoplKind::PortIo),
            Model::Popf => Cpl3Class::IoplSensitive(IoplKind::Popf),
            Model::CplSpecific => Cpl3Class::CplSpecific,
            Model::NotAtCpl3 => Cpl3Class::NotAtCpl3,
        }
    }
}

/// Plan C1-7 item 2: operations that the SDM calls `Neutral` but `cpl3_class` puts into `NotAtCpl3`
/// because the set does not execute them at CPL0 (ADR-0023: "operations of the narrow profiles";
/// plan question V2). Empty in C1; filled only by the result of C2a-2 with its reason.
const NEUTRAL_BY_SDM_NOT_EXECUTED_BY_THE_SET: &[(&str, &str)] = &[];

/// The legacy prefix, REX, map, opcode and ModRM of an encoding.
struct Parsed {
    lock: bool,
    map: &'static str,
    opcode: u8,
    modrm: Option<u8>,
}

fn parse(bytes: &[u8]) -> Parsed {
    let mut index = 0;
    let mut lock = false;
    // Legacy prefixes and REX bytes in any order (a REX that is not directly before the opcode is
    // ignored, Vol. 2A §2.2.1; for the class only the opcode counts).
    while matches!(
        bytes[index],
        0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x40..=0x4f
    ) {
        lock |= bytes[index] == 0xf0;
        index += 1;
    }
    let (map, start) = match (bytes[index], bytes.get(index + 1)) {
        (0x0f, Some(0x38)) => ("0F38", index + 2),
        (0x0f, Some(0x3a)) => ("0F3A", index + 2),
        (0x0f, _) => ("0F", index + 1),
        _ => ("P", index),
    };
    Parsed {
        lock,
        map,
        opcode: bytes[start],
        modrm: bytes.get(start + 1).copied(),
    }
}

/// The model of the opcode maps (SDM Vol. 2, Appendix A, and the instruction pages named in the
/// module doc). The ModRM byte of a form without one is never read.
fn sdm_class(bytes: &[u8]) -> Model {
    let parsed = parse(bytes);
    let reg = parsed.modrm.map(|modrm| (modrm >> 3) & 7);
    let register_form = parsed.modrm.is_some_and(|modrm| modrm >> 6 == 3);
    match (parsed.map, parsed.opcode) {
        // Vol. 3A Table 2-3: HLT.
        ("P", 0xf4) => Model::Privileged,
        // Vol. 2A "CLI", Vol. 2B "STI".
        ("P", 0xfa | 0xfb) => Model::InterruptFlag,
        // Vol. 2A "IN", "INS", Vol. 2B "OUT", "OUTS".
        ("P", 0xe4..=0xe7 | 0xec..=0xef | 0x6c..=0x6f) => Model::PortIo,
        // Vol. 2B "POPF".
        ("P", 0x9d) => Model::Popf,
        // Vol. 2A "IRET", "INT n/INTO/INT3/INT1", "MOV" (to Sreg).
        ("P", 0xcf | 0xcc | 0xcd | 0x8e) => Model::CplSpecific,
        // Far returns (ADR-0023: `NotAtCpl3`).
        ("P", 0xca | 0xcb) => Model::NotAtCpl3,
        // Vol. 2B "SLDT", "STR" (UMIP); Vol. 3A Table 2-3 LLDT, LTR; Vol. 2B "VERR/VERW".
        ("0F", 0x00) => match reg {
            Some(0 | 1) => Model::Umip,
            Some(2 | 3) => Model::Privileged,
            _ => Model::CplSpecific,
        },
        ("0F", 0x01) => match (reg, register_form, parsed.modrm) {
            // Vol. 2B "SWAPGS" (0F 01 F8).
            (_, true, Some(0xf8)) => Model::Privileged,
            // Vol. 2B "RDTSCP" (0F 01 F9): CR4.TSD.
            (_, true, Some(0xf9)) => Model::Tsd,
            // SGDT, SIDT, SMSW (UMIP).
            (Some(0 | 1 | 4), false, _) | (Some(4), true, _) => Model::Umip,
            // LGDT, LIDT, LMSW, INVLPG (Table 2-3).
            (Some(2 | 3 | 7), false, _) | (Some(6), _, _) => Model::Privileged,
            _ => Model::Neutral,
        },
        // Vol. 2A "LAR", Vol. 2B "LSL".
        ("0F", 0x02 | 0x03) => Model::CplSpecific,
        // Vol. 2B "SYSCALL".
        ("0F", 0x05) => Model::CplSpecific,
        // CLTS, SYSRET, INVD, WBINVD.
        ("0F", 0x06 | 0x07 | 0x08 | 0x09) => Model::Privileged,
        // MOV CRn, MOV DRn.
        ("0F", 0x20..=0x23) => Model::Privileged,
        // WRMSR, RDMSR, SYSEXIT.
        ("0F", 0x30 | 0x32 | 0x35) => Model::Privileged,
        ("0F", 0x31) => Model::Tsd,
        ("0F", 0x33) => Model::Pce,
        // SYSENTER (plan question V12).
        ("0F", 0x34) => Model::NotAtCpl3,
        // Vol. 2B "POP" FS/GS.
        ("0F", 0xa1 | 0xa9) => Model::CplSpecific,
        _ => Model::Neutral,
    }
}

#[test]
fn classification_of_every_decoded_encoding_matches_the_sdm_model() {
    assert!(NEUTRAL_BY_SDM_NOT_EXECUTED_BY_THE_SET.is_empty());
    let mut units = Vec::new();
    for (_, map_bytes) in MAPS {
        for opcode in 0..=255u8 {
            units.push((map_bytes, opcode));
        }
    }
    const LEGACY: [&[u8]; 8] = [
        &[],
        &[0x66],
        &[0xf2],
        &[0xf3],
        &[0x67],
        &[0x64],
        &[0x65],
        &[0xf0],
    ];
    let results = cpl3_sweep::parallel_map_with(
        &units,
        || (),
        |_, (map_bytes, opcode)| {
            let mut checked = 0u64;
            let mut mismatches = Vec::new();
            cpl3_sweep::full_sweep_opcode(
                map_bytes,
                *opcode,
                &LEGACY,
                &mut |bytes, instruction| {
                    checked += 1;
                    let name = format!("{:?}", instruction.operation);
                    let variant = name.split([' ', '(']).next().unwrap().to_string();
                    let expected = if NEUTRAL_BY_SDM_NOT_EXECUTED_BY_THE_SET
                        .iter()
                        .any(|(listed, _)| *listed == variant)
                    {
                        Cpl3Class::NotAtCpl3
                    } else {
                        sdm_class(bytes).class()
                    };
                    let actual = cpl3_class(&instruction.operation);
                    if actual != expected && mismatches.len() < 4 {
                        mismatches.push(format!(
                            "{}: {variant} is {actual:?}, the model says {expected:?}",
                            bytes
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>()
                        ));
                    }
                },
            );
            (checked, mismatches)
        },
    );
    let checked: u64 = results.iter().map(|(checked, _)| checked).sum();
    let mismatches: Vec<String> = results.into_iter().flat_map(|(_, found)| found).collect();
    println!("decoded encodings checked: {checked}");
    assert!(checked > 100_000, "{checked}");
    assert!(
        mismatches.is_empty(),
        "{} mismatches (at most 4 per opcode):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

// ---- the outcomes at CPL3 ----------------------------------------------------------------------

/// The qword the code would read at `address` before the run (on-demand pages hold the pattern).
fn initial_qword(memory: &SweepMemory, address: u64) -> u64 {
    memory.peek_u64(address).unwrap_or_else(|| {
        u64::from_le_bytes(core::array::from_fn(|index| {
            pattern(address + index as u64)
        }))
    })
}

/// A literal reason for an `Unsupported` at CPL3 (plan C1-7 item 4): a predicate over the form and
/// the initial state and memory.
type Reason = (
    &'static str,
    fn(&[u8], &LongModeState, &SweepMemory) -> bool,
);

fn opcode_is(bytes: &[u8], map: &str, opcodes: &[u8]) -> bool {
    let parsed = parse(bytes);
    parsed.map == map && opcodes.contains(&parsed.opcode)
}

fn has_legacy_prefix(bytes: &[u8]) -> bool {
    matches!(
        bytes[0],
        0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3
    )
}

fn has_rex(bytes: &[u8]) -> bool {
    let parsed_start = bytes
        .iter()
        .position(|byte| {
            !matches!(
                byte,
                0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x40
                    ..=0x4f
            )
        })
        .unwrap();
    bytes[..parsed_start].iter().any(|byte| byte & 0xf0 == 0x40)
}

const ALLOWED_UNSUPPORTED_REASONS: &[Reason] = &[
    ("IRETQ: TF in the frame image", |bytes, state, memory| {
        opcode_is(bytes, "P", &[0xcf])
            && initial_qword(memory, state.gpr[Register::Rsp as usize] + 16) & (1 << 8) != 0
    }),
    (
        "IRETQ: CS or SS of the frame with TI = 1",
        |bytes, state, memory| {
            let rsp = state.gpr[Register::Rsp as usize];
            opcode_is(bytes, "P", &[0xcf])
                && (initial_qword(memory, rsp + 8) & 4 != 0
                    || initial_qword(memory, rsp + 32) & 4 != 0)
        },
    ),
    (
        "IRETQ: a CS of the frame with L = 0 (compatibility mode)",
        |bytes, state, memory| {
            let cs = initial_qword(memory, state.gpr[Register::Rsp as usize] + 8) as u16;
            opcode_is(bytes, "P", &[0xcf])
                && u64::from(cs & !7) + 7 <= 0x57
                && initial_qword(memory, cpl3_sweep::GDT + u64::from(cs & !7)) & (1 << 53) == 0
        },
    ),
    (
        "IRET without REX.W (IRETD) is not decoded as IRETQ: the decoder form",
        |bytes, _, _| opcode_is(bytes, "P", &[0xcf]) && !has_rex(bytes),
    ),
    ("POPF: TF in the stack image", |bytes, state, memory| {
        opcode_is(bytes, "P", &[0x9d])
            && initial_qword(memory, state.gpr[Register::Rsp as usize]) & (1 << 8) != 0
    }),
    (
        "IN/OUT: allowed by the TSS bit map (the CPL3 entry has no port)",
        |bytes, state, _| {
            let parsed = parse(bytes);
            let port = match parsed.opcode {
                0xe4..=0xe7 => u16::from(parsed.modrm.unwrap_or(0)),
                _ => state.gpr[Register::Rdx as usize] as u16,
            };
            parsed.map == "P"
                && matches!(parsed.opcode, 0xe4..=0xe7 | 0xec..=0xef)
                && ALLOWED_PORTS.iter().any(|range| range.contains(&port))
        },
    ),
    (
        "IN/OUT: CPL <= IOPL (the CPL3 entry has no port)",
        |bytes, state, _| {
            opcode_is(
                bytes,
                "P",
                &[0xe4, 0xe5, 0xe6, 0xe7, 0xec, 0xed, 0xee, 0xef],
            ) && (state.rflags >> 12) & 3 == 3
        },
    ),
    (
        "CLI/STI/INT3/SYSCALL/RDTSC with a prefix: the executors admit the bare form only",
        |bytes, _, _| {
            (opcode_is(bytes, "P", &[0xfa, 0xfb, 0xcc]) || opcode_is(bytes, "0F", &[0x05, 0x31]))
                && (has_legacy_prefix(bytes) || has_rex(bytes))
        },
    ),
    (
        "a segment load with TI = 1 (the LDT is not modeled)",
        |bytes, state, memory| {
            let parsed = parse(bytes);
            if parsed.map != "P" || parsed.opcode != 0x8e {
                return false;
            }
            let selector = match parsed.modrm {
                Some(modrm) if modrm >> 6 == 3 => {
                    let rex_b = has_rex(bytes)
                        && bytes[bytes.iter().position(|b| b & 0xf0 == 0x40).unwrap()] & 1 != 0;
                    state.gpr[usize::from(modrm & 7) | (usize::from(rex_b) << 3)] as u16
                }
                _ => initial_qword(memory, effective_address(bytes, state)) as u16,
            };
            parsed.map == "P" && parsed.opcode == 0x8e && selector & 4 != 0
        },
    ),
];

/// The effective address of the memory forms of the reduced sweep (`[base]`, `[base+disp8]`,
/// `[rip+disp32]`, `[base+index*scale+disp8]`), REX.B/X included.
fn effective_address(bytes: &[u8], state: &LongModeState) -> u64 {
    let rex = bytes
        .iter()
        .take_while(|byte| {
            matches!(
                byte,
                0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x40
                    ..=0x4f
            )
        })
        .filter(|byte| *byte & 0xf0 == 0x40)
        .last()
        .copied()
        .unwrap_or(0);
    let parsed_at = bytes
        .iter()
        .position(|byte| {
            !matches!(
                byte,
                0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x40
                    ..=0x4f
            )
        })
        .unwrap();
    let modrm_at = parsed_at + if bytes[parsed_at] == 0x0f { 2 } else { 1 };
    let modrm = bytes[modrm_at];
    let (mode, rm) = (modrm >> 6, modrm & 7);
    let gpr = |encoding: u8| state.gpr[usize::from(encoding)];
    if mode == 0 && rm == 5 {
        let disp = i32::from_le_bytes(bytes[modrm_at + 1..modrm_at + 5].try_into().unwrap());
        return (RIP + bytes.len() as u64).wrapping_add(disp as i64 as u64);
    }
    if rm == 4 {
        let sib = bytes[modrm_at + 1];
        let base = gpr((sib & 7) | ((rex & 1) << 3));
        let index = ((sib >> 3) & 7) | ((rex & 2) << 2);
        let scaled = if index == 4 {
            0
        } else {
            gpr(index) << (sib >> 6)
        };
        let disp = if mode == 1 {
            bytes[modrm_at + 2] as i8 as i64 as u64
        } else {
            0
        };
        return base.wrapping_add(scaled).wrapping_add(disp);
    }
    let disp = if mode == 1 {
        bytes[modrm_at + 1] as i8 as i64 as u64
    } else {
        0
    };
    gpr(rm | ((rex & 1) << 3)).wrapping_add(disp)
}

/// The CR4 values of the matrix that the set admits with the Windows CPUID member.
const CR4_VALUES: [u64; 4] = [
    CR4,
    CR4 & !(1 << 3),
    CR4 & !((1 << 9) | (1 << 10)),
    0x20 | (1 << 7),
];

/// What the layer must do at CPL3 for a form of the model class `model`.
fn predicted(model: Model, bytes: &[u8], iopl: u8, cr4: u64) -> Option<RunExit> {
    let parsed = parse(bytes);
    if parsed.lock && model != Model::Neutral {
        return Some(UD);
    }
    if parsed.map == "0F" && matches!(parsed.opcode, 0x21 | 0x23) {
        let index = (parsed.modrm.unwrap() >> 3) & 7;
        if matches!(index, 4 | 5) && cr4 & (1 << 3) != 0 {
            return Some(UD);
        }
    }
    // The table of ADR-0023 decision 1 at CPL3 (written here, not taken from `cpl3_gate`).
    let unsupported = Some(RunExit::Unsupported {
        opcode: parsed.opcode,
    });
    match model {
        Model::Privileged => Some(GP0),
        Model::Tsd if cr4 & (1 << 2) != 0 => Some(GP0),
        Model::Pce if cr4 & (1 << 8) == 0 => Some(GP0),
        Model::Umip if cr4 & (1 << 11) != 0 => Some(GP0),
        Model::InterruptFlag if iopl < 3 => Some(GP0),
        Model::PortIo if iopl == 3 => unsupported,
        Model::NotAtCpl3 => unsupported,
        _ => None,
    }
}

fn reason_for(bytes: &[u8], state: &LongModeState, memory: &SweepMemory) -> Option<&'static str> {
    ALLOWED_UNSUPPORTED_REASONS
        .iter()
        .find(|(_, predicate)| predicate(bytes, state, memory))
        .map(|(name, _)| *name)
}

#[test]
fn cpl3_outcomes_match_the_sdm_model() {
    let encodings: Vec<Vec<u8>> = encodings_with_groups()
        .into_keys()
        .filter(|bytes| sdm_class(bytes) != Model::Neutral)
        .collect();
    let failures = cpl3_sweep::parallel_map_with(
        &encodings,
        || (),
        |_, bytes| {
            let mut failures = Vec::new();
            let model = sdm_class(bytes);
            for iopl in 0..4u8 {
                for cr4 in CR4_VALUES {
                    let (state, memory) = machine_cached(3, iopl, cr4);
                    let run: Run = run_on(state.clone(), memory.clone(), Cells::new(), bytes, 1);
                    let exit = run_exit(run.result.as_ref().unwrap()).clone();
                    let name = format!(
                        "{} {model:?} IOPL {iopl} CR4 {cr4:#x}",
                        bytes
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    );
                    if let Some(expected) = predicted(model, bytes, iopl, cr4) {
                        if exit != expected {
                            failures.push(format!("{name}: {exit:?}, the model {expected:?}"));
                        } else if run.state != state || !run.memory.writes.is_empty() {
                            failures.push(format!("{name}: changed the state"));
                        } else if expected == GP0
                            && model != Model::PortIo
                            && !run.memory.accesses.is_empty()
                        {
                            failures.push(format!("{name}: #GP(0) after a memory access"));
                        }
                        continue;
                    }
                    if model == Model::PortIo {
                        // CPL > IOPL: the TSS bit map decides (#GP(0) or the allowed `Unsupported`).
                        let allowed = reason_for(bytes, &state, &memory).is_some();
                        let expected = if allowed {
                            RunExit::Unsupported {
                                opcode: parse(bytes).opcode,
                            }
                        } else {
                            GP0
                        };
                        if exit != expected {
                            failures.push(format!("{name}: {exit:?}, the bit map {expected:?}"));
                        }
                        continue;
                    }
                    match exit {
                        RunExit::InstructionBudgetExhausted
                        | RunExit::StopAddress
                        | RunExit::Fault(_) => {}
                        RunExit::Unsupported { .. } => match reason_for(bytes, &state, &memory) {
                            Some(reason) => failures.push(format!("reason: {reason}")),
                            None => failures
                                .push(format!("{name}: Unsupported for a reason outside the list")),
                        },
                        other => failures.push(format!("{name}: {other:?}")),
                    }
                }
            }
            failures
        },
    );
    let (reasons, failures): (Vec<String>, Vec<String>) = failures
        .into_iter()
        .flatten()
        .partition(|line| line.starts_with("reason: "));
    let mut tally = std::collections::BTreeMap::new();
    for reason in reasons {
        *tally.entry(reason).or_insert(0) += 1;
    }
    for (reason, count) in &tally {
        println!("{count:6} {reason}");
    }
    println!(
        "encodings with a class other than Neutral: {}",
        encodings.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures, first 80:\n{}",
        failures.len(),
        failures
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    let _ = RIP;
}
