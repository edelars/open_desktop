//! ADR-0023 slice C2a-2 (plan `work/s7-32/c1/plan-c1-c2a.md`, C2a-2): every encoding of the class
//! `Neutral` gives the same outcome through the CPL3 entry as through the CPL0 entry of the set.
//!
//! The encodings: the corpus of C2a-1 (`tests/data/c2a_cpl3_legacy_corpus.txt`), the reduced sweep
//! of C1-0 (`cpl3_sweep::encodings`, so that the coverage of every `Neutral` variant does not depend
//! on the reduction of the corpus) and the supplement `tests/data/c2a_neutral_supplement.txt` (up to
//! four encodings of every `Neutral` variant from the classification sweep of C1-7, preferring ones
//! the set executes at CPL0: the narrow forms such as `[rax]`-only or `gs:[disp32]` forms are not in
//! the reduced sweep). The inputs A-D of [`cpl3_sweep::InputVariant`] and two more FP cells on A
//! (an empty x87 stack after FNINIT; two values on the stack), which the x87 forms need (the cell of
//! the sweep has a full stack), and A with a valid FNSAVE image at RAX (FRSTOR, FLDENV); every data and
//! stack page is a user page (U = 1) and CR0.WP = 1; SMAP is not declared, so CPL0 reads and writes
//! the same pages. The set of the sweep (with the FP member), budget 1.
//!
//! Compared: every field of `cpl3_sweep::outcome_fields` (the exit with its payload and the retired
//! count, the whole state, the cells, the FP cell, the write log) and the bytes of every page, except
//! the fields that differ by construction (CS and SS) and the literal list of allowed differences
//! below. Any other difference is a defect, or a new line of the list only after a decision of the
//! coordinator.

mod cpl3_sweep;

use cpl3_sweep::*;
use od_isa::cpl3_class::{Cpl3Class, cpl3_class};
use od_isa::execute::RunExit;
use od_isa::{Cpl0ProfileSetExit, Register};
use std::collections::{BTreeMap, BTreeSet};

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c2a_cpl3_legacy_corpus.txt"
);
const SUPPLEMENT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c2a_neutral_supplement.txt"
);

/// The `Neutral` variants of `cpl3_class` (517 on the code of slice C1, plan appendix A, extracted
/// from `crates/od-isa/src/cpl3_class.rs`, the arms `=> Cpl3Class::Neutral`; 419 after the 98 of
/// [`MOVED_TO_NOT_AT_CPL3`] left the class).
const NEUTRAL_VARIANTS: &[&str] = &[
    "AddFsLoad",
    "AddFsMem",
    "AddFsMemImm",
    "AddGsLoad",
    "AddGsMem",
    "AddGsMemImm",
    "AddpdMemoryRax",
    "AddpdRegister",
    "AddpsMemoryRax",
    "AddpsRegister",
    "AddsdMemoryRax",
    "AddsdRegister",
    "AddssMemoryRax",
    "AddssRegister",
    "AndnpdRegister",
    "AndnpsRegister",
    "AndpdRegister",
    "AndpsRegister",
    "Arithmetic",
    "Bit",
    "BitScan",
    "Branch",
    "Bswap",
    "Call",
    "CarryFlag",
    "CetNop",
    "ClearDirectionFlag",
    "Clflush",
    "CmovFsLoad",
    "CmovGsLoad",
    "CmpFsLoad",
    "CmpFsMem",
    "CmpFsMemImm",
    "CmpGsLoad",
    "CmpGsMem",
    "CmpGsMemImm",
    "CmppdMemoryRax",
    "CmppdRegister",
    "CmppsMemoryRax",
    "CmppsRegister",
    "Cmps",
    "CmpsdMemoryRax",
    "CmpsdRegister",
    "CmpssMemoryRax",
    "CmpssRegister",
    "Cmpxchg",
    "Cmpxchg16b",
    "Cmpxchg8b",
    "CmpxchgFsMem",
    "CmpxchgFsMemByte",
    "CmpxchgGsMem",
    "CmpxchgGsMemByte",
    "CmpxchgMemory",
    "ComisdMemoryRax",
    "ComissMemoryRax",
    "ComissUcomissRegister",
    "ConditionalMove",
    "Cpuid",
    "Crc32",
    "Cvtdq2pdMemoryRax",
    "Cvtdq2pdRegister",
    "Cvtdq2psMemoryRax",
    "Cvtdq2psRegister",
    "Cvtpd2dqMemoryRax",
    "Cvtpd2dqRegister",
    "Cvtpd2psMemoryRax",
    "Cvtpd2psRegister",
    "Cvtps2dqMemoryRax",
    "Cvtps2dqRegister",
    "Cvtps2pdMemoryRax",
    "Cvtps2pdRegister",
    "Cvtsd2siR32MemoryRax",
    "Cvtsd2siR32Register",
    "Cvtsd2siR64MemoryRax",
    "Cvtsd2siR64Register",
    "Cvtsd2ssMemoryRax",
    "Cvtsd2ssRegister",
    "Cvtsi2sdR32MemoryRax",
    "Cvtsi2sdR32Register",
    "Cvtsi2sdR64MemoryRax",
    "Cvtsi2sdR64Register",
    "Cvtsi2ssR32MemoryRax",
    "Cvtsi2ssR32Register",
    "Cvtsi2ssR64MemoryRax",
    "Cvtsi2ssR64Register",
    "Cvtss2sdMemoryRax",
    "Cvtss2sdRegister",
    "Cvtss2siR32MemoryRax",
    "Cvtss2siR32Register",
    "Cvtss2siR64MemoryRax",
    "Cvtss2siR64Register",
    "Cvttpd2dqMemoryRax",
    "Cvttpd2dqRegister",
    "Cvttps2dqMemoryRax",
    "Cvttps2dqRegister",
    "Cvttsd2siR32MemoryRax",
    "Cvttsd2siR32Register",
    "Cvttsd2siR64MemoryRax",
    "Cvttsd2siR64Register",
    "Cvttss2siR32MemoryRax",
    "Cvttss2siR32Register",
    "Cvttss2siR64MemoryRax",
    "Cvttss2siR64Register",
    "Divide",
    "DivpdMemoryRax",
    "DivpdRegister",
    "DivpsMemoryRax",
    "DivpsRegister",
    "DivsdMemoryRax",
    "DivsdRegister",
    "DivssMemoryRax",
    "DivssRegister",
    "Emms",
    "Endbr64Nop",
    "Extend",
    "FfreeSti",
    "FildM32Int",
    "FildM32IntRax",
    "FldenvRax",
    "Fnclex",
    "Fninit",
    "FnsaveMemory",
    "FnsaveRax",
    "FnstcwM16",
    "FnstenvRax",
    "FnstswAx",
    "FnstswM16",
    "FrstorRax",
    "Fwait",
    "Fxrstor",
    "Fxrstor64RbxDisp8",
    "Fxrstor64Rdi",
    "Fxrstor64RipRelative",
    "Fxsave",
    "Fxsave64R12",
    "Fxsave64RaxDisp8",
    "FxsaveRipRelative",
    "Imul",
    "ImulImmediate",
    "IncDecFsMem",
    "IncDecGsMem",
    "IndirectCall",
    "IndirectJump",
    "Jrcxz",
    "Jump",
    "Lahf",
    "LdmxcsrMemory",
    "Lea",
    "Leave",
    "Lfence",
    "LockAddMem",
    "LockAddMemReg",
    "LockAndMem",
    "LockBitMem",
    "LockBitMemImm",
    "LockCmpxchgMem",
    "LockDecMem",
    "LockIncMem",
    "LockLogicMemReg",
    "LockOrMem",
    "LockSubDwordImm8",
    "LockSubMem",
    "LockSubMemReg",
    "LockXaddMem",
    "LockXorMem",
    "LockedRmw",
    "Lods",
    "Logic",
    "LogicFsLoad",
    "LogicFsMemImm",
    "LogicGsLoad",
    "LogicGsMemImm",
    "Loop",
    "Loope",
    "Loopne",
    "MaxpdMemoryRax",
    "MaxpdRegisterMasked",
    "MaxpsMemoryRax",
    "MaxpsRegisterMasked",
    "MaxsdMemoryRax",
    "MaxsdRegisterMasked",
    "MaxssMemoryRax",
    "MaxssRegisterMasked",
    "Mfence",
    "MinpdMemoryRax",
    "MinpdRegisterMasked",
    "MinpsMemoryRax",
    "MinpsRegisterMasked",
    "MinsdMemoryRax",
    "MinsdRegisterMasked",
    "MinssMemoryRax",
    "MinssRegisterMasked",
    "MmxInt",
    "MmxMovd",
    "MmxMovq",
    "MmxShiftImmediate",
    "MmxSsse3",
    "Mov",
    "MovFsLoad",
    "MovFsLoadD",
    "MovFsLoadQ",
    "MovFsMemByteImm",
    "MovFsMemImm",
    "MovFsSelectorToRax",
    "MovFsStore",
    "MovGsLoad",
    "MovGsLoadD",
    "MovGsLoadQ",
    "MovGsMemByteImm",
    "MovGsMemImm",
    "MovGsSelectorToRax",
    "MovGsStore",
    "MovImmediate",
    "MovSregStore",
    "MovapdRegister",
    "MovapdReverseRegister",
    "MovapsLoadMemory",
    "MovapsRegister",
    "MovapsReverseRegister",
    "MovapsStoreMemory",
    "MovdqaLoadMemory",
    "MovdqaRegister",
    "MovdquLoadMemory",
    "MoveIntegerToXmm",
    "MovhpdLoadMemory",
    "MovhpsLoadMemory",
    "MovlpdLoadMemory",
    "MovqFromXmm",
    "Movs",
    "MovsdLoadMemory",
    "MovsdRegister",
    "MovsdStoreMemory",
    "MovsdStoreRegister",
    "MovssLoadMemory",
    "MovssRegister",
    "MovssStoreMemory",
    "MovsxFsLoadB",
    "MovsxGsLoadB",
    "MovsxdFsLoad",
    "MovsxdGsLoad",
    "MovupdLoadMemory",
    "MovupdRegister",
    "MovupdReverseRegister",
    "MovupdStoreMemory",
    "MovupsLoadMemory",
    "MovupsRegister",
    "MovupsStoreMemory",
    "MovzxFsLoadB",
    "MovzxFsLoadW",
    "MovzxGsLoadB",
    "MovzxGsLoadW",
    "Mul",
    "MulpdMemoryRax",
    "MulpdRegister",
    "MulpsMemoryRax",
    "MulpsRegister",
    "MulsdMemory",
    "MulsdMemoryRax",
    "MulsdRegister",
    "MulssMemoryRax",
    "MulssRegister",
    "Nop",
    "NopModrm",
    "OrFsMem",
    "OrGsMem",
    "OrpdRegister",
    "OrpsRegister",
    "PackssdwRegister",
    "PacksswbRegister",
    "PackuswbRegister",
    "PaddbRegister",
    "PadddRegister",
    "PaddqRegister",
    "PaddsbRegister",
    "PaddswRegister",
    "PaddusbRegister",
    "PadduswRegister",
    "PaddwRegister",
    "PandRegister",
    "PandnRegister",
    "Pause",
    "PavgbRegister",
    "PavgwRegister",
    "PcmpeqbMemory",
    "PcmpeqbRegister",
    "PcmpeqdRegister",
    "PcmpeqwRegister",
    "PcmpgtbRegister",
    "PcmpgtdRegister",
    "PcmpgtwRegister",
    "PinsrwMemory",
    "PmaddwdRegister",
    "PmaxswRegister",
    "PmaxubRegister",
    "PminswRegister",
    "PminubMemory",
    "PminubRegister",
    "PmovmskbXmmToGpr",
    "PmulhuwRegister",
    "PmullwRegister",
    "PmuludqRegister",
    "PopMemory",
    "Popcnt",
    "PorRegister",
    "Prefetchw",
    "PsadbwRegister",
    "PshufdRegister",
    "PshuflwRegister",
    "PslldImmediateRegister",
    "PslldqImmediateRegister",
    "PsllqImmediateRegister",
    "PsllwImmediateRegister",
    "PsradImmediateRegister",
    "PsrawImmediateRegister",
    "PsrldImmediateRegister",
    "PsrldqImmediateRegister",
    "PsrlqImmediateRegister",
    "PsrlwImmediateRegister",
    "PsubbRegister",
    "PsubdRegister",
    "PsubqRegister",
    "PsubsbRegister",
    "PsubswRegister",
    "PsubusbRegister",
    "PsubuswRegister",
    "PsubwRegister",
    "PunpckhbwRegister",
    "PunpckhdqRegister",
    "PunpckhqdqRegister",
    "PunpckhwdRegister",
    "PunpcklbwRegister",
    "PunpckldqRegister",
    "PunpcklqdqRegister",
    "PunpcklwdRegister",
    "PushFlags",
    "PushFsMem",
    "PushGsMem",
    "PushImmediate",
    "PushMemory",
    "PxorRegister",
    "RcppsMemoryRax",
    "RcppsRegister",
    "RcpssMemoryRax",
    "RcpssRegister",
    "RdsspqRax",
    "RepCmps",
    "RepLods",
    "RepMovs",
    "RepScas",
    "RepStos",
    "RepneScas",
    "RepneScasWdqAddr32",
    "RepneScasbAddr32",
    "Return",
    "RsqrtpsMemoryRax",
    "RsqrtpsRegister",
    "RsqrtssMemoryRax",
    "RsqrtssRegister",
    "Sahf",
    "Scas",
    "SetCondition",
    "SetDirectionFlag",
    "Sfence",
    "Shift",
    "SignExtendAccumulator",
    "SqrtpdMemoryRax",
    "SqrtpdRegister",
    "SqrtpsMemoryRax",
    "SqrtpsRegister",
    "SqrtsdMemoryRax",
    "SqrtsdRegister",
    "SqrtssMemoryRax",
    "SqrtssRegister",
    "Sse128MemoryMove",
    "Sse2Int",
    "Sse2Maskmov",
    "Sse2MmxMove",
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
    "Stmxcsr",
    "Stos",
    "SubFsMem",
    "SubFsMemImm",
    "SubGsMem",
    "SubGsMemImm",
    "SubpdMemoryRax",
    "SubpdRegister",
    "SubpsMemoryRax",
    "SubpsRegister",
    "SubsdMemoryRax",
    "SubsdRegister",
    "SubssMemoryRax",
    "SubssRegister",
    "TestFsMemByte",
    "TestFsMemWide",
    "TestFsMemWord",
    "TestGsMemByte",
    "TestGsMemWide",
    "TestGsMemWord",
    "UcomisdMemoryRax",
    "UcomissMemoryRax",
    "Xadd",
    "XaddDsMem",
    "XaddFsMem",
    "XaddGsMem",
    "XaddMemory",
    "Xchg",
    "XchgMemory",
    "XorpdRegister",
    "XorpsRegister",
];

/// The `Neutral` variants of slice C1 that the set does not execute at CPL0 in any encoding of the
/// corpus, the reduced sweep and the supplement on any input (the result of
/// [`every_neutral_variant_is_covered`] on the code of slice C2a before the reclassification) and
/// that therefore moved to `NotAtCpl3` (plan question V2; the reasons are the list of
/// `c1_cpl3_privilege_model.rs`, `NEUTRAL_BY_SDM_NOT_EXECUTED_BY_THE_SET`).
const MOVED_TO_NOT_AT_CPL3: &[&str] = &[
    "Cvtpd2piMm0MemoryRax",
    "Cvtpd2piMm0Xmm1",
    "Cvtpi2pdMemoryRax",
    "Cvtpi2pdXmm0Mm1",
    "Cvtpi2psMemoryRax",
    "Cvtpi2psXmm0Mm1",
    "Cvtps2piMm0MemoryRax",
    "Cvtps2piMm0Xmm1",
    "Cvttpd2piMm0MemoryRax",
    "Cvttpd2piMm0Xmm1",
    "Cvttps2piMm0MemoryRax",
    "Cvttps2piMm0Xmm1",
    "F2xm1",
    "Fabs",
    "FaddSt0St",
    "FaddStSt0",
    "FaddpStSt0",
    "FbldM80Rax",
    "FbstpM80Rax",
    "Fchs",
    "Fcmovb",
    "Fcmovbe",
    "Fcmove",
    "Fcmovnb",
    "Fcmovnbe",
    "Fcmovne",
    "Fcmovnu",
    "Fcmovu",
    "FcomSt",
    "Fcomi",
    "Fcomip",
    "FcompSt",
    "Fcompp",
    "Fcos",
    "Fdecstp",
    "FdivSt0St",
    "FdivStSt0",
    "FdivpStSt0",
    "FdivrSt0St",
    "FdivrStSt0",
    "FdivrpStSt0",
    "FildM16IntRax",
    "FildM64IntRax",
    "Fincstp",
    "FistM16IntRax",
    "FistM32IntRax",
    "FistpM16IntRax",
    "FistpM32IntRax",
    "FistpM64IntRax",
    "FisttpMemory",
    "Fld1",
    "FldM32FpRax",
    "FldM64FpRax",
    "FldM80FpRax",
    "FldSti",
    "FldcwM16Rax",
    "Fldz",
    "FmulSt0St",
    "FmulStSt0",
    "FmulpStSt0",
    "Fnop",
    "FnstcwM16Rax",
    "FnstswM16Rax",
    "Fpatan",
    "Fprem",
    "Fprem1",
    "Fptan",
    "Frndint",
    "Fscale",
    "Fsin",
    "Fsincos",
    "Fsqrt",
    "FstM32FpRax",
    "FstM64FpRax",
    "FstSti",
    "FstpM32FpRax",
    "FstpM64FpRax",
    "FstpM80FpRax",
    "FstpSti",
    "FsubSt0St",
    "FsubStSt0",
    "FsubpStSt0",
    "FsubrSt0St",
    "FsubrStSt0",
    "FsubrpStSt0",
    "Ftst",
    "Fucom",
    "Fucomi",
    "Fucomip",
    "Fucomp",
    "Fucompp",
    "Fxam",
    "FxchSti",
    "Fxtract",
    "Fyl2x",
    "Fyl2xp1",
    "RepneCmpsWdqAddr32",
    "RepneCmpsbAddr32",
];

/// The corpus and the reduced sweep, the `Neutral` encodings only, unique and sorted.
fn neutral_encodings() -> Vec<Vec<u8>> {
    let mut all: BTreeSet<Vec<u8>> = std::fs::read_to_string(CORPUS)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| unhex(line.split_whitespace().next().unwrap()))
        .collect();
    all.extend(encodings());
    all.extend(
        std::fs::read_to_string(SUPPLEMENT)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .map(|line| unhex(line.split_whitespace().next().unwrap())),
    );
    all.extend(NARROW_FORMS.iter().map(|bytes| bytes.to_vec()));
    all.into_iter()
        .filter(|bytes| {
            decode(bytes)
                .is_ok_and(|instruction| cpl3_class(&instruction.operation) == Cpl3Class::Neutral)
        })
        .collect()
}

/// Encodings of narrow variants that neither the corpus nor the sweeps produce (exact
/// displacements of the decoder, a DS override), plan C2a-2 item 3 (i).
const NARROW_FORMS: &[&[u8]] = &[
    // FXSAVE64 [RAX+0x40] (`Fxsave64RaxDisp8`), FXRSTOR64 [RBX+0x40] (`Fxrstor64RbxDisp8`).
    &[0x48, 0x0f, 0xae, 0x40, 0x40],
    &[0x48, 0x0f, 0xae, 0x4b, 0x40],
    // FXRSTOR64 [RIP+0x01CAC498] (`Fxrstor64RipRelative`).
    &[0x48, 0x0f, 0xae, 0x0d, 0x98, 0xc4, 0xca, 0x01],
    // DS: XADD [RDI], EAX and the qword form (`XaddDsMem`).
    &[0x3e, 0x0f, 0xc1, 0x07],
    &[0x3e, 0x48, 0x0f, 0xc1, 0x07],
];

/// The result of one encoding: its variant, the differences on every input and whether the set
/// executes it at CPL0 on some input (not `Unsupported`).
struct EncodingResult {
    name: String,
    failures: Vec<String>,
    executes_at_cpl0: bool,
}

/// The results of every encoding, computed once for both tests.
fn results() -> &'static (usize, Vec<EncodingResult>) {
    static RESULTS: std::sync::OnceLock<(usize, Vec<EncodingResult>)> = std::sync::OnceLock::new();
    RESULTS.get_or_init(|| {
        let encodings = neutral_encodings();
        let results = parallel_map_with(
            &encodings,
            || (),
            |_, bytes| {
                let name = operation_name(bytes);
                let mut failures = Vec::new();
                let mut executes_at_cpl0 = false;
                for input in DIFF_INPUTS {
                    let (differences, cpl0) = differences(bytes, input);
                    executes_at_cpl0 |= !unsupported(&cpl0);
                    if !differences.is_empty() {
                        failures.push(format!(
                            "{} {name} {input:?}: {}",
                            hex(bytes),
                            differences
                                .iter()
                                .take(3)
                                .map(|(field, cpl0, cpl3)| format!(
                                    "{field}: cpl0 {cpl0} | cpl3 {cpl3}"
                                ))
                                .collect::<Vec<_>>()
                                .join("; ")
                        ));
                    }
                }
                EncodingResult {
                    name,
                    failures,
                    executes_at_cpl0,
                }
            },
        );
        (encodings.len(), results)
    })
}

#[test]
fn neutral_forms_at_cpl3_equal_the_cpl0_set() {
    let (count, results) = results();
    let failures: Vec<&String> = results.iter().flat_map(|result| &result.failures).collect();
    if let Ok(path) = std::env::var("OD_C2A_DIFFERENTIAL_OUT") {
        let text: Vec<&str> = failures.iter().map(|failure| failure.as_str()).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }
    assert!(
        failures.is_empty(),
        "{} differences in {count} Neutral encodings x {} inputs; first:\n{}",
        failures.len(),
        DIFF_INPUTS.len(),
        failures
            .iter()
            .take(40)
            .map(|failure| failure.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_neutral_variant_is_covered() {
    let (_, results) = results();
    let mut by_variant: BTreeMap<&str, bool> = BTreeMap::new();
    for result in results {
        *by_variant.entry(&result.name).or_default() |= result.executes_at_cpl0;
    }
    let listed: BTreeSet<&str> = NEUTRAL_VARIANTS.iter().copied().collect();
    let unlisted: Vec<&&str> = by_variant
        .keys()
        .filter(|name| !listed.contains(**name))
        .collect();
    assert!(
        unlisted.is_empty(),
        "Neutral variants outside the literal list: {unlisted:?}"
    );
    let uncovered: Vec<&&str> = NEUTRAL_VARIANTS
        .iter()
        .filter(|name| !by_variant.get(**name).copied().unwrap_or(false))
        .collect();
    let without_encoding: Vec<&&&str> = uncovered
        .iter()
        .filter(|name| !by_variant.contains_key(***name))
        .collect();
    assert!(
        uncovered.is_empty(),
        "{} Neutral variants without an encoding the set executes at CPL0 ({} without any \
         encoding: {without_encoding:?}): {uncovered:?}",
        uncovered.len(),
        without_encoding.len()
    );
}

/// The supplement of plan C2a-2 item 3 (i): for every `Neutral` variant up to four encodings of the
/// classification sweep of C1-7 (`full_classification_sweep`), the ones the set executes at CPL0
/// on some input first (of the first 64 candidates of the variant). Deterministic.
#[test]
#[ignore = "generator: OD_C2A_WRITE_SUPPLEMENT=<commit> writes tests/data/c2a_neutral_supplement.txt"]
fn write_neutral_supplement() {
    // Review F7: written on the code of slice C1 (in a clean `git archive` copy): on the code of C2a the
    // variants that left `Neutral` (`MOVED_TO_NOT_AT_CPL3`) would lose their encodings.
    let Ok(commit) = std::env::var("OD_C2A_WRITE_SUPPLEMENT") else {
        return;
    };
    let mut candidates: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    let opcodes: Vec<(usize, u8)> = (0..MAPS.len())
        .flat_map(|map| (0..=255u8).map(move |opcode| (map, opcode)))
        .collect();
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
    let found = parallel_map_with(
        &opcodes,
        || (),
        |_, &(map, opcode)| {
            let mut local: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
            full_sweep_opcode(MAPS[map].1, opcode, &LEGACY, &mut |bytes, instruction| {
                if cpl3_class(&instruction.operation) != Cpl3Class::Neutral {
                    return;
                }
                let name = format!("{:?}", instruction.operation)
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let list = local.entry(name).or_default();
                if list.len() < 64 && !list.iter().any(|known| known == bytes) {
                    list.push(bytes.to_vec());
                }
            });
            local
        },
    );
    for local in found {
        for (name, list) in local {
            let all = candidates.entry(name).or_default();
            for bytes in list {
                if all.len() < 64 && !all.contains(&bytes) {
                    all.push(bytes);
                }
            }
        }
    }
    let entries: Vec<(String, Vec<Vec<u8>>)> = candidates.into_iter().collect();
    let chosen = parallel_map_with(
        &entries,
        || (),
        |_, (name, list)| {
            let (mut runs, mut refused): (Vec<&Vec<u8>>, Vec<&Vec<u8>>) = (Vec::new(), Vec::new());
            for bytes in list {
                let executes = DIFF_INPUTS
                    .iter()
                    .any(|&input| !unsupported(&side(bytes, 0, input)));
                if executes {
                    runs.push(bytes)
                } else {
                    refused.push(bytes)
                }
            }
            runs.into_iter()
                .chain(refused)
                .take(4)
                .map(|bytes| format!("{} {name}\n", hex(bytes)))
                .collect::<String>()
        },
    );
    std::fs::write(
        SUPPLEMENT,
        format!(
            "# ADR-0023 slice C2a-2: up to four encodings of every Neutral variant from the classification\n\
             # sweep of C1-7, the ones the set executes at CPL0 first (`write_neutral_supplement`).\n\
             # Recorded on {commit} (slice C1), a clean `git archive` copy with the test files.\n"
        ) + &chosen.concat(),
    )
    .unwrap();
}

/// Plan C2a-4 item 1: the CPL3 entry decodes an instruction once: the fetches of one `48 8b d9`
/// equal those of one direct `long_decode::decode` on the same memory.
#[test]
fn the_cpl3_entry_decodes_each_instruction_once() {
    let bytes = [0x48, 0x8b, 0xd9];
    let (state, mut memory) = machine(3, 0, CR4);
    memory.put(state.rip, &bytes);
    let mut probe = memory.clone();
    od_isa::long_decode::decode(
        &mut probe,
        state.rip,
        context(od_isa::PrivilegeLevel::Ring3),
    )
    .unwrap();
    let one_decode = probe.fetches;
    let run = run_on(state, memory, Cells::new(), &bytes, 1);
    assert!(
        matches!(
            run.result,
            Ok(ref result) if result.exit == Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
        ),
        "{:?}",
        run.result
    );
    assert_eq!(
        run.memory.fetches, one_decode,
        "fetches of the CPL3 entry for one instruction"
    );
}

/// Plan C2a-2 item 3 (ii): every variant of [`MOVED_TO_NOT_AT_CPL3`] is `NotAtCpl3` now, and each of
/// its encodings in the supplement (four per variant, written on the code of slice C1) is still
/// `Unsupported` at CPL0 on every input (the reason of the move holds) and at CPL3 (the class).
#[test]
fn the_neutral_variants_the_set_does_not_execute_are_not_at_cpl3() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let supplement = std::fs::read_to_string(SUPPLEMENT).unwrap();
    for line in supplement
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
    {
        let bytes = unhex(line.split_whitespace().next().unwrap());
        let name = operation_name(&bytes);
        let Some(moved) = MOVED_TO_NOT_AT_CPL3.iter().find(|moved| **moved == name) else {
            continue;
        };
        *seen.entry(moved).or_default() += 1;
        let instruction = decode(&bytes).unwrap();
        assert_eq!(
            cpl3_class(&instruction.operation),
            Cpl3Class::NotAtCpl3,
            "{name}"
        );
        for input in DIFF_INPUTS {
            for cpl in [0, 3] {
                let run = side(&bytes, cpl, input);
                assert!(
                    unsupported(&run),
                    "{} {name} CPL{cpl} {input:?}: {:?}",
                    hex(&bytes),
                    run.result
                );
                if cpl == 3 {
                    let (state, _) = input_machine(3, CR4, input.0);
                    assert_eq!(run.state, state, "{} {name}: changed at CPL3", hex(&bytes));
                }
            }
        }
    }
    let missing: Vec<&&str> = MOVED_TO_NOT_AT_CPL3
        .iter()
        .filter(|name| !seen.contains_key(**name))
        .collect();
    assert!(
        missing.is_empty(),
        "moved variants without a supplement encoding: {missing:?}"
    );
}

/// ISSUE-727 (ADR-0023 decision 1, slice C2a): `67 E8 rel32` at a high canonical RIP, at CPL0 (kernel
/// half, through the CPL0 entry) and CPL3 (top of the user half, through the CPL3 entry), with a
/// positive and a negative rel32: the target is `RIP_next + rel32` in 64 bits (no truncation to 32 by
/// the address-size prefix), the push is the 8-byte return address at `RSP - 8` in 64 bits (Vol. 1
/// §3.6.1, Vol. 2A "CALL": 67h acts on memory operands and rCX/rSI/rDI only).
#[test]
fn an_addr32_call_rel32_targets_and_pushes_in_64_bits_at_cpl0_and_cpl3() {
    for (cpl, rip, rsp) in [
        (0u8, 0xffff_f800_0000_1000u64, 0xffff_f800_0010_0800u64),
        (3, 0x0000_7fff_0000_1000, 0x0000_7fff_0010_0800),
    ] {
        for rel32 in [0x1000_0000i32, -0x2000_0000] {
            let (mut state, mut memory) = machine(cpl, 0, CR4);
            memory.map(rip, cpl == 3, true);
            memory.map(rsp - 8, cpl == 3, true);
            state.rip = rip;
            state.gpr[Register::Rsp as usize] = rsp;
            let mut bytes = vec![0x67, 0xe8];
            bytes.extend(rel32.to_le_bytes());
            let run = run_on(state, memory, Cells::new(), &bytes, 1);
            let label = format!("CPL{cpl} rel32 {rel32:#x}");
            assert!(
                matches!(
                    &run.result,
                    Ok(result) if result.exit == Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                        && result.instructions_executed == 1
                ),
                "{label}: {:?}",
                run.result
            );
            let next = rip + 6;
            assert_eq!(
                run.state.rip,
                next.wrapping_add(rel32 as i64 as u64),
                "{label}"
            );
            assert_eq!(run.state.gpr[Register::Rsp as usize], rsp - 8, "{label}");
            assert_eq!(run.memory.peek_u64(rsp - 8), Some(next), "{label}");
            assert_eq!(
                run.memory.writes,
                vec![(rsp - 8, next.to_le_bytes().to_vec())],
                "{label}"
            );
        }
    }
}

/// Review F2: the forms whose destination read faults with W = 1 at CPL3 and W = 0 at CPL0 (input E:
/// the data page not present) are exactly the forms of the literal list `cpl3_write_intent_form` (the
/// table of `cpl3_write_intent_rmw_destination`): every row of the table is exercised and no form
/// outside it differs.
#[test]
fn write_intent_forms_are_exactly_the_literal_list() {
    let encodings = neutral_encodings();
    let rows = parallel_map_with(
        &encodings,
        || (),
        |_, bytes| {
            let code = |run: &Run| match &run.result {
                Ok(result) => match &result.exit {
                    Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) if fault.vector == 14 => {
                        fault.error_code
                    }
                    _ => None,
                },
                Err(_) => None,
            };
            let input = (InputVariant::E, FpCell::Seed);
            let (cpl0, cpl3) = (side(bytes, 0, input), side(bytes, 3, input));
            let differs = matches!((code(&cpl0), code(&cpl3)), (Some(c0), Some(c3)) if c0 & 2 == 0 && c3 & 2 != 0);
            (bytes.clone(), differs, cpl3_write_intent_form(bytes))
        },
    );
    let outside: Vec<String> = rows
        .iter()
        .filter(|(_, differs, listed)| *differs && !listed)
        .map(|(bytes, _, _)| hex(bytes))
        .collect();
    assert!(
        outside.is_empty(),
        "W = 1 at CPL3 outside the literal list: {outside:?}"
    );
    // Review G4: a row of the literal list is a template (opcode, ModRM.reg or any, operand sizes) as
    // `cpl3_write_intent_form` writes it, one row per opcode; for the forms with a register source
    // (`01`, `29`, `20`, `09`) ModRM.reg is the source and is not part of the row.
    const ROWS: [(u8, Option<&[u8]>, &[usize]); 17] = [
        (0x83, Some(&[0]), &[2, 4, 8]),
        (0x83, Some(&[5]), &[4]),
        (0x01, None, &[4, 8]),
        (0x29, None, &[4, 8]),
        (0xfe, Some(&[0]), &[1]),
        (0xff, Some(&[0]), &[4, 8]),
        (0xff, Some(&[1]), &[4]),
        (0x80, Some(&[4]), &[1]),
        (0x20, None, &[1]),
        (0x81, Some(&[1, 4]), &[4]),
        (0x83, Some(&[1, 4]), &[4]),
        (0x83, Some(&[1, 4]), &[8]),
        (0x81, Some(&[4]), &[2]),
        (0x83, Some(&[4]), &[2]),
        (0x81, Some(&[6]), &[4]),
        (0x09, None, &[8]),
        (0xd0, Some(&[7]), &[1]),
    ];
    let mut exercised = BTreeSet::new();
    let mut unmatched = Vec::new();
    for (bytes, differs, listed) in &rows {
        if *differs && *listed {
            let instruction = decode(bytes).unwrap();
            let layout = od_isa::length::long64_layout(bytes).unwrap();
            let reg = (bytes[layout.modrm.unwrap()] >> 3) & 7;
            let (opcode, size) = (instruction.opcode.byte, instruction.operand_size.bytes());
            let matched: Vec<usize> = ROWS
                .iter()
                .enumerate()
                .filter(|(_, (row_opcode, regs, sizes))| {
                    *row_opcode == opcode
                        && regs.is_none_or(|regs| regs.contains(&reg))
                        && sizes.contains(&size)
                })
                .map(|(index, _)| index)
                .collect();
            if matched.is_empty() {
                unmatched.push(hex(bytes));
            }
            exercised.extend(matched);
        }
    }
    assert!(
        unmatched.is_empty(),
        "listed forms outside the rows: {unmatched:?}"
    );
    let missing: Vec<_> = (0..ROWS.len())
        .filter(|index| !exercised.contains(index))
        .map(|index| ROWS[index])
        .collect();
    assert!(
        missing.is_empty(),
        "rows of the literal list not exercised: {missing:x?}"
    );
}
