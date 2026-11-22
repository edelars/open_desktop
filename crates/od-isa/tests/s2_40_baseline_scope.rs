//! S2-40 (ADR-0015 item 2): scope check of the x86-64 baseline forms. Each encoding of the baseline
//! families (SSE, SSE2, SSE3, SSSE3, SSE4.1, SSE4.2, POPCNT, CRC32, CMPXCHG16B, LAHF/SAHF, PREFETCHW)
//! is fed, in a register and a memory form, to two executors: the bare `step` (no control-register
//! policy) and one instruction of the CPL0 profile set with the FP member (CR0/CR4 gates open). The
//! encodings are from the Intel SDM Vol. 2A..2D opcode maps, not from the interpreter. This is a
//! presence check only: semantics are covered by the per-task suites (s2_51..s2_56) and the sse_fp,
//! sse2_int and mmx_int modules of the library, and the CPL3 refusal of these forms by
//! cpl3_profile_set.rs. The forms that an executor
//! refuses today are listed in `KNOWN_UNSUPPORTED_BARE` / `KNOWN_UNSUPPORTED_CPL0_SET`; the test fails
//! if a form off the list is refused or a form on the list starts to execute (update the lists and the
//! S2-40 block of the plan then).

use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig,
    RunExit, StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};
use std::cell::RefCell;

const CODE: u64 = 0x2000;
const DATA: u64 = 0x8010;
const RAM: usize = 0x10000;

const SPEC: &str = "
# SSE (prerequisite forms)
MOVUPS|0F10|B
MOVUPS-store|0F11|B
MOVLPS|0F12|M
MOVHLPS|0F12|R
MOVLPS-store|0F13|M
UNPCKLPS|0F14|B
UNPCKHPS|0F15|B
MOVHPS|0F16|M
MOVLHPS|0F16|R
MOVHPS-store|0F17|M
MOVAPS|0F28|B
MOVAPS-store|0F29|B
CVTPI2PS|0F2A|B
CVTSI2SS|F30F2A|B
MOVNTPS|0F2B|M
CVTTPS2PI|0F2C|B
CVTTSS2SI|F30F2C|B
CVTPS2PI|0F2D|B
CVTSS2SI|F30F2D|B
UCOMISS|0F2E|B
COMISS|0F2F|B
MOVMSKPS|0F50|R
SQRTPS|0F51|B
SQRTSS|F30F51|B
RSQRTPS|0F52|B
RSQRTSS|F30F52|B
RCPPS|0F53|B
RCPSS|F30F53|B
ANDPS|0F54|B
ANDNPS|0F55|B
ORPS|0F56|B
XORPS|0F57|B
ADDPS|0F58|B
ADDSS|F30F58|B
MULPS|0F59|B
MULSS|F30F59|B
SUBPS|0F5C|B
SUBSS|F30F5C|B
MINPS|0F5D|B
MINSS|F30F5D|B
DIVPS|0F5E|B
DIVSS|F30F5E|B
MAXPS|0F5F|B
MAXSS|F30F5F|B
CMPPS|0FC2|Bi
CMPSS|F30FC2|Bi
SHUFPS|0FC6|Bi
MOVSS|F30F10|B
MOVSS-store|F30F11|B
LDMXCSR|0FAE|M2
STMXCSR|0FAE|M3
SFENCE|0FAEF8|N
PREFETCHNTA|0F18|M0
PREFETCHT0|0F18|M1
PREFETCHT1|0F18|M2
PREFETCHT2|0F18|M3
PSHUFW|0F70|Bi
PINSRW-mmx|0FC4|Bi
PEXTRW-mmx|0FC5|Ri
PMOVMSKB-mmx|0FD7|R
MASKMOVQ|0FF7|R
MOVNTQ|0FE7|M
PAVGB-mmx|0FE0|B
PAVGW-mmx|0FE3|B
PMULHUW-mmx|0FE4|B
PMINUB-mmx|0FDA|B
PMAXUB-mmx|0FDE|B
PMINSW-mmx|0FEA|B
PMAXSW-mmx|0FEE|B
PSADBW-mmx|0FF6|B
# SSE2
MOVUPD|660F10|B
MOVUPD-store|660F11|B
MOVSD|F20F10|B
MOVSD-store|F20F11|B
MOVLPD|660F12|M
MOVLPD-store|660F13|M
UNPCKLPD|660F14|B
UNPCKHPD|660F15|B
MOVHPD|660F16|M
MOVHPD-store|660F17|M
MOVAPD|660F28|B
MOVAPD-store|660F29|B
CVTPI2PD|660F2A|B
CVTSI2SD|F20F2A|B
MOVNTPD|660F2B|M
CVTTPD2PI|660F2C|B
CVTTSD2SI|F20F2C|B
CVTPD2PI|660F2D|B
CVTSD2SI|F20F2D|B
UCOMISD|660F2E|B
COMISD|660F2F|B
MOVMSKPD|660F50|R
SQRTPD|660F51|B
SQRTSD|F20F51|B
ANDPD|660F54|B
ANDNPD|660F55|B
ORPD|660F56|B
XORPD|660F57|B
ADDPD|660F58|B
ADDSD|F20F58|B
MULPD|660F59|B
MULSD|F20F59|B
CVTPS2PD|0F5A|B
CVTPD2PS|660F5A|B
CVTSS2SD|F30F5A|B
CVTSD2SS|F20F5A|B
CVTDQ2PS|0F5B|B
CVTPS2DQ|660F5B|B
CVTTPS2DQ|F30F5B|B
SUBPD|660F5C|B
SUBSD|F20F5C|B
MINPD|660F5D|B
MINSD|F20F5D|B
DIVPD|660F5E|B
DIVSD|F20F5E|B
MAXPD|660F5F|B
MAXSD|F20F5F|B
PUNPCKLBW|660F60|B
PUNPCKLWD|660F61|B
PUNPCKLDQ|660F62|B
PACKSSWB|660F63|B
PCMPGTB|660F64|B
PCMPGTW|660F65|B
PCMPGTD|660F66|B
PACKUSWB|660F67|B
PUNPCKHBW|660F68|B
PUNPCKHWD|660F69|B
PUNPCKHDQ|660F6A|B
PACKSSDW|660F6B|B
PUNPCKLQDQ|660F6C|B
PUNPCKHQDQ|660F6D|B
MOVD-to-xmm|660F6E|B
MOVDQA|660F6F|B
MOVDQU|F30F6F|B
PSHUFD|660F70|Bi
PSHUFHW|F30F70|Bi
PSHUFLW|F20F70|Bi
PSRLW-imm|660F71|G2i
PSRAW-imm|660F71|G4i
PSLLW-imm|660F71|G6i
PSRLD-imm|660F72|G2i
PSRAD-imm|660F72|G4i
PSLLD-imm|660F72|G6i
PSRLQ-imm|660F73|G2i
PSRLDQ|660F73|G3i
PSLLQ-imm|660F73|G6i
PSLLDQ|660F73|G7i
PCMPEQB|660F74|B
PCMPEQW|660F75|B
PCMPEQD|660F76|B
MOVD-from-xmm|660F7E|B
MOVQ-load|F30F7E|B
MOVDQA-store|660F7F|B
MOVDQU-store|F30F7F|B
CMPPD|660FC2|Bi
CMPSD|F20FC2|Bi
MOVNTI|0FC3|M
PINSRW|660FC4|Bi
PEXTRW|660FC5|Ri
SHUFPD|660FC6|Bi
CLFLUSH|0FAE|M7
LFENCE|0FAEE8|N
MFENCE|0FAEF0|N
PAUSE|F390|N
PSRLW|660FD1|B
PSRLD|660FD2|B
PSRLQ|660FD3|B
PADDQ|660FD4|B
PMULLW|660FD5|B
MOVQ-store|660FD6|B
PMOVMSKB|660FD7|R
PSUBUSB|660FD8|B
PSUBUSW|660FD9|B
PMINUB|660FDA|B
PAND|660FDB|B
PADDUSB|660FDC|B
PADDUSW|660FDD|B
PMAXUB|660FDE|B
PANDN|660FDF|B
PAVGB|660FE0|B
PSRAW|660FE1|B
PSRAD|660FE2|B
PAVGW|660FE3|B
PMULHUW|660FE4|B
PMULHW|660FE5|B
CVTTPD2DQ|660FE6|B
CVTPD2DQ|F20FE6|B
CVTDQ2PD|F30FE6|B
MOVNTDQ|660FE7|M
PSUBSB|660FE8|B
PSUBSW|660FE9|B
PMINSW|660FEA|B
POR|660FEB|B
PADDSB|660FEC|B
PADDSW|660FED|B
PMAXSW|660FEE|B
PXOR|660FEF|B
PSLLW|660FF1|B
PSLLD|660FF2|B
PSLLQ|660FF3|B
PMULUDQ|660FF4|B
PMADDWD|660FF5|B
PSADBW|660FF6|B
MASKMOVDQU|660FF7|R
PSUBB|660FF8|B
PSUBW|660FF9|B
PSUBD|660FFA|B
PSUBQ|660FFB|B
PADDB|660FFC|B
PADDW|660FFD|B
PADDD|660FFE|B
PADDQ-mmx|0FD4|B
PMULUDQ-mmx|0FF4|B
PSUBQ-mmx|0FFB|B
MOVQ2DQ|F30FD6|R
MOVDQ2Q|F20FD6|R
# SSE3
ADDSUBPD|660FD0|B
ADDSUBPS|F20FD0|B
HADDPD|660F7C|B
HADDPS|F20F7C|B
HSUBPD|660F7D|B
HSUBPS|F20F7D|B
LDDQU|F20FF0|M
MOVDDUP|F20F12|B
MOVSHDUP|F30F16|B
MOVSLDUP|F30F12|B
FISTTP-m16|DF|M1
FISTTP-m32|DB|M1
FISTTP-m64|DD|M1
# SSSE3 (XMM and MMX)
PSHUFB|660F3800|B
PHADDW|660F3801|B
PHADDD|660F3802|B
PHADDSW|660F3803|B
PMADDUBSW|660F3804|B
PHSUBW|660F3805|B
PHSUBD|660F3806|B
PHSUBSW|660F3807|B
PSIGNB|660F3808|B
PSIGNW|660F3809|B
PSIGND|660F380A|B
PMULHRSW|660F380B|B
PABSB|660F381C|B
PABSW|660F381D|B
PABSD|660F381E|B
PALIGNR|660F3A0F|Bi
PSHUFB-mmx|0F3800|B
PHADDW-mmx|0F3801|B
PHADDD-mmx|0F3802|B
PHADDSW-mmx|0F3803|B
PMADDUBSW-mmx|0F3804|B
PHSUBW-mmx|0F3805|B
PHSUBD-mmx|0F3806|B
PHSUBSW-mmx|0F3807|B
PSIGNB-mmx|0F3808|B
PSIGNW-mmx|0F3809|B
PSIGND-mmx|0F380A|B
PMULHRSW-mmx|0F380B|B
PABSB-mmx|0F381C|B
PABSW-mmx|0F381D|B
PABSD-mmx|0F381E|B
PALIGNR-mmx|0F3A0F|Bi
# SSE4.1
BLENDPS|660F3A0C|Bi
BLENDPD|660F3A0D|Bi
PBLENDW|660F3A0E|Bi
PBLENDVB|660F3810|B
BLENDVPS|660F3814|B
BLENDVPD|660F3815|B
PTEST|660F3817|B
PMOVSXBW|660F3820|B
PMOVSXBD|660F3821|B
PMOVSXBQ|660F3822|B
PMOVSXWD|660F3823|B
PMOVSXWQ|660F3824|B
PMOVSXDQ|660F3825|B
PMULDQ|660F3828|B
PCMPEQQ|660F3829|B
MOVNTDQA|660F382A|M
PACKUSDW|660F382B|B
PMOVZXBW|660F3830|B
PMOVZXBD|660F3831|B
PMOVZXBQ|660F3832|B
PMOVZXWD|660F3833|B
PMOVZXWQ|660F3834|B
PMOVZXDQ|660F3835|B
PMINSB|660F3838|B
PMINSD|660F3839|B
PMINUW|660F383A|B
PMINUD|660F383B|B
PMAXSB|660F383C|B
PMAXSD|660F383D|B
PMAXUW|660F383E|B
PMAXUD|660F383F|B
PMULLD|660F3840|B
PHMINPOSUW|660F3841|B
ROUNDPS|660F3A08|Bi
ROUNDPD|660F3A09|Bi
ROUNDSS|660F3A0A|Bi
ROUNDSD|660F3A0B|Bi
DPPS|660F3A40|Bi
DPPD|660F3A41|Bi
MPSADBW|660F3A42|Bi
INSERTPS|660F3A21|Bi
EXTRACTPS|660F3A17|Bi
PEXTRB|660F3A14|Bi
PEXTRW-sse41|660F3A15|Bi
PEXTRD|660F3A16|Bi
PINSRB|660F3A20|Bi
PINSRD|660F3A22|Bi
PEXTRQ|66480F3A16|Bi
PINSRQ|66480F3A22|Bi
# SSE4.2
PCMPESTRM|660F3A60|Bi
PCMPESTRI|660F3A61|Bi
PCMPISTRM|660F3A62|Bi
PCMPISTRI|660F3A63|Bi
PCMPGTQ|660F3837|B
CRC32-r8|F20F38F0|B
CRC32-r32|F20F38F1|B
CRC32-r64|F2480F38F1|B
POPCNT-r16|66F30FB8|B
POPCNT-r32|F30FB8|B
POPCNT-r64|F3480FB8|B
# Other baseline scope
CMPXCHG16B|480FC7|M1
CMPXCHG16B-lock|F0480FC7|M1
LAHF|9F|N
SAHF|9E|N
PREFETCHW|0F0D|M1
";

/// Forms that the bare step refuses ("name (form)"). Most of them are in the CPL0 set (fences, prefetch,
/// MOVD/MOVQ, MOVLPD ...) or are narrow-profile forms of S2-34 that decode with a [RAX] base only.
const KNOWN_UNSUPPORTED_BARE: &[&str] = &[
    "MOVHPS (mem)",
    "MOVSS (mem)",
    "MOVSS-store (mem)",
    "LDMXCSR (mem)",
    "STMXCSR (mem)",
    "SFENCE (none)",
    "PREFETCHNTA (mem)",
    "PREFETCHT0 (mem)",
    "PREFETCHT1 (mem)",
    "PREFETCHT2 (mem)",
    "PINSRW-mmx (reg)",
    "PINSRW-mmx (mem)",
    "PEXTRW-mmx (reg)",
    "PMOVMSKB-mmx (reg)",
    "MASKMOVQ (reg)",
    "MOVNTQ (mem)",
    "PAVGB-mmx (reg)",
    "PAVGB-mmx (mem)",
    "PAVGW-mmx (reg)",
    "PAVGW-mmx (mem)",
    "PMULHUW-mmx (reg)",
    "PMULHUW-mmx (mem)",
    "PMINUB-mmx (reg)",
    "PMINUB-mmx (mem)",
    "PMAXUB-mmx (reg)",
    "PMAXUB-mmx (mem)",
    "PMINSW-mmx (reg)",
    "PMINSW-mmx (mem)",
    "PMAXSW-mmx (reg)",
    "PMAXSW-mmx (mem)",
    "PSADBW-mmx (reg)",
    "PSADBW-mmx (mem)",
    "PADDQ-mmx (reg)",
    "PADDQ-mmx (mem)",
    "PMULUDQ-mmx (reg)",
    "PMULUDQ-mmx (mem)",
    "PSUBQ-mmx (reg)",
    "PSUBQ-mmx (mem)",
    "MOVSD (mem)",
    "MOVSD-store (mem)",
    "MOVLPD (mem)",
    "MOVHPD (mem)",
    "MOVD-to-xmm (reg)",
    "MOVD-to-xmm (mem)",
    "PSHUFLW (reg)",
    "PCMPEQB (mem)",
    "MOVQ-load (mem)",
    "MOVNTI (mem)",
    "PINSRW (mem)",
    "CLFLUSH (mem)",
    "LFENCE (none)",
    "PAUSE (none)",
    "MOVQ-store (mem)",
];

/// Forms that the CPL0 profile set with the FP member refuses. Refused by both lists (no executor runs
/// them): MOVSS/MOVSD with a base other than [RAX], MOVNTI, CLFLUSH, the SSE integer forms on MMX
/// registers (PAVGB/PAVGW/PMULHUW/PMINUB/PMAXUB/PMINSW/PMAXSW/PSADBW/PINSRW/PEXTRW/PMOVMSKB),
/// MOVNTQ, MASKMOVQ and the MMX forms of PADDQ/PSUBQ/PMULUDQ. None of the forms refused by both belongs
/// to a family whose CPUID bit the Windows profile declares. The forms refused only by the set are the
/// twelve MMX conversions (SSE/SSE2, bits not declared) and FISTTP, which belongs to SSE3: the Windows
/// profile declares that bit (discrepancy 2 of the S2-40 block of the plan).
const KNOWN_UNSUPPORTED_CPL0_SET: &[&str] = &[
    "CVTPI2PS (reg)",
    "CVTPI2PS (mem)",
    "CVTTPS2PI (reg)",
    "CVTTPS2PI (mem)",
    "CVTPS2PI (reg)",
    "CVTPS2PI (mem)",
    "MOVSS (mem)",
    "MOVSS-store (mem)",
    "PINSRW-mmx (reg)",
    "PINSRW-mmx (mem)",
    "PEXTRW-mmx (reg)",
    "PMOVMSKB-mmx (reg)",
    "MASKMOVQ (reg)",
    "MOVNTQ (mem)",
    "PAVGB-mmx (reg)",
    "PAVGB-mmx (mem)",
    "PAVGW-mmx (reg)",
    "PAVGW-mmx (mem)",
    "PMULHUW-mmx (reg)",
    "PMULHUW-mmx (mem)",
    "PMINUB-mmx (reg)",
    "PMINUB-mmx (mem)",
    "PMAXUB-mmx (reg)",
    "PMAXUB-mmx (mem)",
    "PMINSW-mmx (reg)",
    "PMINSW-mmx (mem)",
    "PMAXSW-mmx (reg)",
    "PMAXSW-mmx (mem)",
    "PSADBW-mmx (reg)",
    "PSADBW-mmx (mem)",
    "PADDQ-mmx (reg)",
    "PADDQ-mmx (mem)",
    "PMULUDQ-mmx (reg)",
    "PMULUDQ-mmx (mem)",
    "PSUBQ-mmx (reg)",
    "PSUBQ-mmx (mem)",
    "MOVSD (mem)",
    "MOVSD-store (mem)",
    "CVTPI2PD (reg)",
    "CVTPI2PD (mem)",
    "CVTTPD2PI (reg)",
    "CVTTPD2PI (mem)",
    "CVTPD2PI (reg)",
    "CVTPD2PI (mem)",
    "MOVNTI (mem)",
    "CLFLUSH (mem)",
    "FISTTP-m16 (mem)",
    "FISTTP-m32 (mem)",
    "FISTTP-m64 (mem)",
];

const CR0_BASE: u64 = 0x8001_0033;
const CR4_FP: u64 = 0x20 | (1 << 9) | (1 << 10);

struct Ram {
    bytes: Vec<u8>,
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let start = address as usize;
        let end = start + out.len();
        if end > RAM {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            });
        }
        out.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        let start = address as usize;
        let end = start + data.len();
        if end > RAM {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            });
        }
        self.bytes[start..end].copy_from_slice(data);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn cpu() -> LongModeState {
    let mut cpu = LongModeState {
        rip: CODE,
        rflags: 0x2 | 0xcd5,
        x87: Some(X87State::fixture_seed()),
        sse: Some(SseState {
            xmm: core::array::from_fn(|index| {
                core::array::from_fn(|lane| {
                    (index as u8).wrapping_mul(0x11) ^ (lane as u8).wrapping_mul(7) ^ 0x35
                })
            }),
            mxcsr: 0x1f80,
        }),
        ..LongModeState::default()
    };
    for (index, slot) in cpu.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    cpu.gpr[Register::Rdi as usize] = DATA;
    cpu
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

/// The byte strings that a SPEC shape stands for: the register form (ModRM C1 or the group digit),
/// the [rdi] memory form (ModRM 07 or the group digit), an optional zero immediate.
fn encodings(opcode: &[u8], shape: &str) -> Vec<(&'static str, Vec<u8>)> {
    let immediate = shape.ends_with('i');
    let body = shape.trim_end_matches('i');
    let mut forms = Vec::new();
    let mut push = |label: &'static str, modrm: Option<u8>| {
        let mut bytes = opcode.to_vec();
        if let Some(modrm) = modrm {
            bytes.push(modrm);
        }
        if immediate {
            bytes.push(if label == "reg" && body.starts_with('G') {
                1
            } else {
                0
            });
        }
        forms.push((label, bytes));
    };
    match body.as_bytes()[0] {
        b'N' => push("none", None),
        b'R' => push("reg", Some(0xc1)),
        b'M' if body.len() > 1 => push("mem", Some(((body.as_bytes()[1] - b'0') << 3) | 7)),
        b'M' => push("mem", Some(0x07)),
        b'B' => {
            push("reg", Some(0xc1));
            push("mem", Some(0x07));
        }
        b'G' => push("reg", Some(0xc0 | ((body.as_bytes()[1] - b'0') << 3) | 1)),
        _ => unreachable!("shape {shape}"),
    }
    forms
}

fn context() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

fn memory(bytes: &[u8]) -> Ram {
    let mut ram = Ram {
        bytes: vec![0x5a; RAM],
    };
    ram.bytes[CODE as usize..CODE as usize + bytes.len()].copy_from_slice(bytes);
    ram
}

fn run_bare(bytes: &[u8]) -> StepOutcome {
    let mut ram = memory(bytes);
    let mut cpu = cpu();
    execute::step(&mut cpu, &mut ram, context())
        .unwrap_or_else(|error| panic!("memory error {error:?} for {bytes:02x?}"))
}

/// One instruction in the CPL0 profile set with the FP member, CR0/CR4 gates open, CPUID profile of
/// the Linux board (the Windows profile does not declare the FP/FXSR/SSE bits that the FP member needs).
fn run_cpl0_set(bytes: &[u8]) -> RunExit {
    let mut ram = memory(bytes);
    let seed = cpu();
    let mut state = LongModeState {
        rip: CODE,
        rflags: 2 | (1 << 9) | 0x85,
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_FP),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        gpr: seed.gpr,
        ..LongModeState::default()
    };
    state.gpr[Register::Rdi as usize] = DATA;
    let fp = seed.sse.unwrap();
    let mut x87 = X87State::fixture_seed();
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    let cell = RefCell::new(GuestFpState { x87, sse: fp });
    let profile = CpuidProfile::X8664V1Stage4A;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&profile)
        .with_fp_state(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: context(),
        },
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap_or_else(|error| panic!("set input error {error:?} for {bytes:02x?}"));
    match result.exit {
        Cpl0ProfileSetExit::Run(exit) => exit,
        other => panic!("unexpected set exit {other:?} for {bytes:02x?}"),
    }
}

/// Outcome of the survey. A refusal is `Unsupported`; a `#UD` (vector 6) is not expected from any form of
/// the baseline and is collected apart, so that it is never taken for a refusal or for presence.
struct Survey {
    total: usize,
    bare: Vec<String>,
    set: Vec<String>,
    undefined: Vec<String>,
}

fn is_undefined(fault: &ArchitecturalFault) -> bool {
    fault.vector == 6
}

fn survey() -> Survey {
    let mut total = 0;
    let (mut bare, mut set, mut undefined) = (Vec::new(), Vec::new(), Vec::new());
    let verbose = std::env::var("S2_40_SURVEY").is_ok();
    for line in SPEC
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut parts = line.split('|');
        let (name, opcode, shape) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        for (label, bytes) in encodings(&hex(opcode), shape) {
            total += 1;
            let tag = format!("{name} ({label}) {bytes:02x?}");
            let outcome = run_bare(&bytes);
            if verbose {
                println!("bare {tag}: {outcome:?}");
            }
            match &outcome {
                StepOutcome::Unsupported { .. } => bare.push(tag.clone()),
                StepOutcome::Fault(fault) if is_undefined(fault) => {
                    undefined.push(format!("bare {tag}"));
                }
                _ => {}
            }
            let exit = run_cpl0_set(&bytes);
            if verbose {
                println!("set  {tag}: {exit:?}");
            }
            match &exit {
                RunExit::Unsupported { .. } => set.push(tag),
                RunExit::Fault(fault) if is_undefined(fault) => {
                    undefined.push(format!("set {tag}"));
                }
                _ => {}
            }
        }
    }
    Survey {
        total,
        bare,
        set,
        undefined,
    }
}

fn unexpected<'a>(refused: &'a [String], known: &[&str]) -> Vec<&'a String> {
    refused
        .iter()
        .filter(|line| !known.iter().any(|name| line.starts_with(name)))
        .collect()
}

#[test]
fn refused_baseline_forms_are_exactly_the_recorded_list() {
    let Survey {
        total,
        bare,
        set,
        undefined,
    } = survey();
    println!(
        "forms checked: {total}; bare refused: {}; CPL0 set refused: {}",
        bare.len(),
        set.len()
    );
    for line in &bare {
        println!("BARE REFUSED {line}");
    }
    for line in &set {
        println!("SET REFUSED {line}");
    }
    assert_eq!(
        total, 596,
        "number of baseline encodings checked (SPEC changed?)"
    );
    assert!(
        undefined.is_empty(),
        "#UD (vector 6) from a baseline form; it is neither a refusal nor presence: {undefined:#?}"
    );
    let bare_new = unexpected(&bare, KNOWN_UNSUPPORTED_BARE);
    let set_new = unexpected(&set, KNOWN_UNSUPPORTED_CPL0_SET);
    assert!(
        bare_new.is_empty() && set_new.is_empty(),
        "refused but not on the list; bare: {bare_new:#?}\nset: {set_new:#?}"
    );
    let stale = |known: &[&'static str], refused: &[String]| {
        known
            .iter()
            .filter(|name| !refused.iter().any(|line| line.starts_with(**name)))
            .copied()
            .collect::<Vec<_>>()
    };
    let (bare_stale, set_stale) = (
        stale(KNOWN_UNSUPPORTED_BARE, &bare),
        stale(KNOWN_UNSUPPORTED_CPL0_SET, &set),
    );
    assert!(
        bare_stale.is_empty() && set_stale.is_empty(),
        "on the list but executed now (update the list and the plan block); bare: {bare_stale:?}\nset: {set_stale:?}"
    );
}
