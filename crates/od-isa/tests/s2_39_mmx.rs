//! S2-39 (MMX): the integer MMX instructions on the MMX registers MM0..MM7 (the low quadwords of the physical x87
//! data registers): PADDB/W/D, PADDSB/SW, PADDUSB/USW, PSUBB/W/D, PSUBSB/SW, PSUBUSB/USW, PAND, PANDN, POR, PXOR,
//! PCMPEQB/W/D, PCMPGTB/W/D, PMULLW, PMULHW, PMADDWD, PUNPCKL/H BW/WD/DQ, PACKSSWB, PACKUSWB, PACKSSDW,
//! PSLL/PSRL W/D/Q, PSRAW/D (register and imm8 counts), PSHUFW, and MOVD/MOVQ between an MMX register and a general
//! register or memory (NP 0F 6E/7E/6F/7F).
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200): Vol. 1 §9.2.2, §9.4, §9.5, §9.6.2
//! (data types, wraparound/saturation, MMX registers and their effect on the x87 state); Vol. 2A-2D, the
//! instruction page of every mnemonic above, "Operation" of the 64-bit versions; Vol. 3A §15.2 Table 15-2. The lane
//! arithmetic below is a second, independent model (widened i128 lanes, no shared code with od_isa::mmx_int). The
//! physical oracle of this slice (Core i5-3210M, 18964 cases) is described in the stage report.

mod mmx_common;

use mmx_common::*;
use od_isa::execute::{RunExit, StepOutcome};
use od_isa::long_decode::{self, DecodeError, Operand, Operation, Sse3Source};
use od_isa::mmx_int::{MmxIntKind, mmx_int_result, mmx_shift_result};
use od_isa::{AccessContext, LongModeState, MemoryError, PrivilegeLevel, Register};

// ---- the independent reference ----

fn unsigned_lanes(value: u64, bits: u32) -> Vec<i128> {
    (0..64 / bits)
        .map(|lane| i128::from((value >> (lane * bits)) & (u64::MAX >> (64 - bits))))
        .collect()
}

fn signed_lanes(value: u64, bits: u32) -> Vec<i128> {
    unsigned_lanes(value, bits)
        .into_iter()
        .map(|lane| {
            if lane >= 1 << (bits - 1) {
                lane - (1 << bits)
            } else {
                lane
            }
        })
        .collect()
}

fn pack(bits: u32, lanes: &[i128]) -> u64 {
    assert_eq!(lanes.len() as u32 * bits, 64);
    lanes.iter().enumerate().fold(0u64, |acc, (index, lane)| {
        let truncated = lane.rem_euclid(1 << bits) as u64;
        acc | truncated << (index as u32 * bits)
    })
}

fn sat_signed(value: i128, bits: u32) -> i128 {
    value.clamp(-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
}

fn sat_unsigned(value: i128, bits: u32) -> i128 {
    value.clamp(0, (1 << bits) - 1)
}

fn bits_of(name: &str) -> u32 {
    match name.chars().last().unwrap() {
        'b' => 8,
        'w' => 16,
        'd' => 32,
        _ => 64,
    }
}

fn reference(name: &str, dest: u64, src: u64, imm: u8) -> u64 {
    let zip = |bits: u32, signed: bool, f: &dyn Fn(i128, i128) -> i128| {
        let (a, b) = if signed {
            (signed_lanes(dest, bits), signed_lanes(src, bits))
        } else {
            (unsigned_lanes(dest, bits), unsigned_lanes(src, bits))
        };
        pack(
            bits,
            &a.iter().zip(&b).map(|(a, b)| f(*a, *b)).collect::<Vec<_>>(),
        )
    };
    let b = bits_of(name);
    match name {
        "paddb" | "paddw" | "paddd" => zip(b, false, &|a, c| a + c),
        "psubb" | "psubw" | "psubd" => zip(b, false, &|a, c| a - c),
        "paddsb" | "paddsw" => zip(b, true, &|a, c| sat_signed(a + c, b)),
        "psubsb" | "psubsw" => zip(b, true, &|a, c| sat_signed(a - c, b)),
        "paddusb" | "paddusw" => zip(b / 1, false, &|a, c| sat_unsigned(a + c, b)),
        "psubusb" | "psubusw" => zip(b, false, &|a, c| sat_unsigned(a - c, b)),
        "pand" => dest & src,
        "pandn" => !dest & src,
        "por" => dest | src,
        "pxor" => dest ^ src,
        "pcmpeqb" | "pcmpeqw" | "pcmpeqd" => zip(b, false, &|a, c| if a == c { -1 } else { 0 }),
        "pcmpgtb" | "pcmpgtw" | "pcmpgtd" => zip(b, true, &|a, c| if a > c { -1 } else { 0 }),
        "pmullw" => zip(16, true, &|a, c| a * c),
        "pmulhw" => zip(16, true, &|a, c| (a * c) >> 16),
        "pmaddwd" => {
            let (a, c) = (signed_lanes(dest, 16), signed_lanes(src, 16));
            pack(32, &[a[0] * c[0] + a[1] * c[1], a[2] * c[2] + a[3] * c[3]])
        }
        "punpcklbw" | "punpckhbw" | "punpcklwd" | "punpckhwd" | "punpckldq" | "punpckhdq" => {
            let bits = match &name[name.len() - 2..] {
                "bw" => 8,
                "wd" => 16,
                _ => 32,
            };
            let (a, c) = (unsigned_lanes(dest, bits), unsigned_lanes(src, bits));
            let half = a.len() / 2;
            let start = if name.starts_with("punpckh") { half } else { 0 };
            let lanes: Vec<i128> = (0..half)
                .flat_map(|i| [a[start + i], c[start + i]])
                .collect();
            pack(bits, &lanes)
        }
        "packsswb" | "packuswb" => {
            let lanes: Vec<i128> = signed_lanes(dest, 16)
                .into_iter()
                .chain(signed_lanes(src, 16))
                .map(|lane| {
                    if name == "packsswb" {
                        sat_signed(lane, 8)
                    } else {
                        sat_unsigned(lane, 8)
                    }
                })
                .collect();
            pack(8, &lanes)
        }
        "packssdw" => {
            let lanes: Vec<i128> = signed_lanes(dest, 32)
                .into_iter()
                .chain(signed_lanes(src, 32))
                .map(|lane| sat_signed(lane, 16))
                .collect();
            pack(16, &lanes)
        }
        "psllw" | "pslld" | "psllq" => {
            let count = u128::from(src);
            let lanes: Vec<i128> = unsigned_lanes(dest, b)
                .into_iter()
                .map(|lane| {
                    if count >= u128::from(b) {
                        0
                    } else {
                        lane << count
                    }
                })
                .collect();
            pack(b, &lanes)
        }
        "psrlw" | "psrld" | "psrlq" => {
            let count = u128::from(src);
            let lanes: Vec<i128> = unsigned_lanes(dest, b)
                .into_iter()
                .map(|lane| {
                    if count >= u128::from(b) {
                        0
                    } else {
                        lane >> count
                    }
                })
                .collect();
            pack(b, &lanes)
        }
        "psraw" | "psrad" => {
            let count = u128::from(src).min(u128::from(b) - 1);
            let lanes: Vec<i128> = signed_lanes(dest, b)
                .into_iter()
                .map(|lane| lane >> count)
                .collect();
            pack(b, &lanes)
        }
        "pshufw" => {
            let words = unsigned_lanes(src, 16);
            let lanes: Vec<i128> = (0..4)
                .map(|i| words[usize::from((imm >> (2 * i)) & 3)])
                .collect();
            pack(16, &lanes)
        }
        other => panic!("no reference for {other}"),
    }
}

/// The no-prefix opcode byte of every two-operand kind (PSHUFW included) with the library kind.
const KINDS: &[(&str, u8, MmxIntKind)] = &[
    ("paddb", 0xfc, MmxIntKind::Paddb),
    ("paddw", 0xfd, MmxIntKind::Paddw),
    ("paddd", 0xfe, MmxIntKind::Paddd),
    ("paddsb", 0xec, MmxIntKind::Paddsb),
    ("paddsw", 0xed, MmxIntKind::Paddsw),
    ("paddusb", 0xdc, MmxIntKind::Paddusb),
    ("paddusw", 0xdd, MmxIntKind::Paddusw),
    ("psubb", 0xf8, MmxIntKind::Psubb),
    ("psubw", 0xf9, MmxIntKind::Psubw),
    ("psubd", 0xfa, MmxIntKind::Psubd),
    ("psubsb", 0xe8, MmxIntKind::Psubsb),
    ("psubsw", 0xe9, MmxIntKind::Psubsw),
    ("psubusb", 0xd8, MmxIntKind::Psubusb),
    ("psubusw", 0xd9, MmxIntKind::Psubusw),
    ("pand", 0xdb, MmxIntKind::Pand),
    ("pandn", 0xdf, MmxIntKind::Pandn),
    ("por", 0xeb, MmxIntKind::Por),
    ("pxor", 0xef, MmxIntKind::Pxor),
    ("pcmpeqb", 0x74, MmxIntKind::Pcmpeqb),
    ("pcmpeqw", 0x75, MmxIntKind::Pcmpeqw),
    ("pcmpeqd", 0x76, MmxIntKind::Pcmpeqd),
    ("pcmpgtb", 0x64, MmxIntKind::Pcmpgtb),
    ("pcmpgtw", 0x65, MmxIntKind::Pcmpgtw),
    ("pcmpgtd", 0x66, MmxIntKind::Pcmpgtd),
    ("pmullw", 0xd5, MmxIntKind::Pmullw),
    ("pmulhw", 0xe5, MmxIntKind::Pmulhw),
    ("pmaddwd", 0xf5, MmxIntKind::Pmaddwd),
    ("punpcklbw", 0x60, MmxIntKind::Punpcklbw),
    ("punpcklwd", 0x61, MmxIntKind::Punpcklwd),
    ("punpckldq", 0x62, MmxIntKind::Punpckldq),
    ("punpckhbw", 0x68, MmxIntKind::Punpckhbw),
    ("punpckhwd", 0x69, MmxIntKind::Punpckhwd),
    ("punpckhdq", 0x6a, MmxIntKind::Punpckhdq),
    ("packsswb", 0x63, MmxIntKind::Packsswb),
    ("packuswb", 0x67, MmxIntKind::Packuswb),
    ("packssdw", 0x6b, MmxIntKind::Packssdw),
    ("psllw", 0xf1, MmxIntKind::Psllw),
    ("pslld", 0xf2, MmxIntKind::Pslld),
    ("psllq", 0xf3, MmxIntKind::Psllq),
    ("psrlw", 0xd1, MmxIntKind::Psrlw),
    ("psrld", 0xd2, MmxIntKind::Psrld),
    ("psrlq", 0xd3, MmxIntKind::Psrlq),
    ("psraw", 0xe1, MmxIntKind::Psraw),
    ("psrad", 0xe2, MmxIntKind::Psrad),
    ("pshufw", 0x70, MmxIntKind::Pshufw),
];

/// Per-lane boundary values of every width, as 64-bit operands with the value in every lane and with the lanes
/// rotating through the table.
fn boundary_operands() -> Vec<u64> {
    const T1: [u64; 11] = [
        0x00, 0x01, 0x02, 0x7e, 0x7f, 0x80, 0x81, 0xfe, 0xff, 0x40, 0xc0,
    ];
    const T2: [u64; 12] = [
        0x0000, 0x0001, 0x7fff, 0x8000, 0x8001, 0xffff, 0x4000, 0xc000, 0x2000, 0x0002, 0xfffe,
        0x3fff,
    ];
    const T3: [u64; 8] = [
        0,
        1,
        0x7fff_ffff,
        0x8000_0000,
        0x8000_0001,
        0xffff_ffff,
        0x4000_0000,
        0xc000_0000,
    ];
    let mut out = vec![];
    for (table, bits) in [(&T1[..], 8), (&T2[..], 16), (&T3[..], 32)] {
        for (index, value) in table.iter().enumerate() {
            out.push((0..64 / bits).fold(0u64, |acc, lane| acc | value << (lane * bits)));
            out.push((0..64 / bits).fold(0u64, |acc, lane| {
                acc | table[(index + lane as usize) % table.len()] << (lane * bits)
            }));
        }
    }
    out
}

fn operand_pairs() -> Vec<(u64, u64)> {
    let boundary = boundary_operands();
    let mut pairs = vec![];
    for (i, a) in boundary.iter().enumerate() {
        for b in boundary.iter().skip(i % 3).step_by(3) {
            pairs.push((*a, *b));
            pairs.push((*b, *a));
        }
    }
    let mut rng = Rng(0x39_aa_55);
    for index in 0..4000 {
        pairs.push((rng.vector(index), rng.vector(index / 3)));
    }
    pairs
}

// ---- the lane arithmetic ----

#[test]
fn every_kind_agrees_with_the_independent_lane_model_on_boundary_and_generated_operands() {
    let pairs = operand_pairs();
    for (name, _, kind) in KINDS {
        for (dest, src) in &pairs {
            for imm in [0x00u8, 0x1b, 0xe4, 0x4e, 0xff] {
                if *name != "pshufw" && imm != 0 {
                    continue;
                }
                let expected = reference(name, *dest, *src, imm);
                let actual = u64::from_le_bytes(mmx_int_result(
                    *kind,
                    dest.to_le_bytes(),
                    src.to_le_bytes(),
                    imm,
                ));
                assert_eq!(
                    actual, expected,
                    "{name} dest={dest:016x} src={src:016x} imm={imm:02x}"
                );
            }
        }
    }
}

#[test]
fn shifts_cover_every_count_through_the_lane_width_and_the_huge_counts_of_a_64_bit_source() {
    let data = [
        0x8123_4567_89ab_cdefu64,
        0xffff_ffff_ffff_ffff,
        0x0001_8000_7fff_fffe,
        0,
    ];
    let counts: Vec<u64> = (0..=130)
        .chain([
            255,
            256,
            1 << 32,
            (1 << 32) + 3,
            (1 << 32) + 63,
            u64::MAX,
            1 << 63,
            0xffff_ffff,
        ])
        .collect();
    for (name, _, kind) in KINDS.iter().filter(|(name, ..)| {
        name.starts_with("psll") || name.starts_with("psrl") || name.starts_with("psra")
    }) {
        for dest in data {
            for count in &counts {
                let expected = reference(name, dest, *count, 0);
                let by_register = u64::from_le_bytes(mmx_int_result(
                    *kind,
                    dest.to_le_bytes(),
                    count.to_le_bytes(),
                    0,
                ));
                assert_eq!(by_register, expected, "{name} {dest:016x} by {count:#x}");
                let by_immediate =
                    u64::from_le_bytes(mmx_shift_result(*kind, dest.to_le_bytes(), *count));
                assert_eq!(by_immediate, expected, "{name} {dest:016x} by {count:#x}");
            }
        }
    }
}

#[test]
fn literal_vectors_of_the_instruction_pages() {
    let bytes = |v: [u8; 8]| u64::from_le_bytes(v);
    let words =
        |v: [u16; 4]| u64::from_le_bytes(core::array::from_fn(|i| v[i / 2].to_le_bytes()[i % 2]));
    let dwords = |v: [u32; 2]| (u64::from(v[1]) << 32) | u64::from(v[0]);
    let run = |kind: MmxIntKind, d: u64, s: u64, imm: u8| {
        u64::from_le_bytes(mmx_int_result(kind, d.to_le_bytes(), s.to_le_bytes(), imm))
    };
    // PADDB wraps, PADDSB clamps to -128/127, PADDUSB clamps to 255.
    let d = bytes([0x7f, 0x80, 0xff, 0x01, 0, 0x40, 0xc0, 0x7e]);
    let s = bytes([0x01, 0xff, 0x01, 0xff, 0, 0x40, 0xc0, 0x02]);
    assert_eq!(
        run(MmxIntKind::Paddb, d, s, 0),
        bytes([0x80, 0x7f, 0x00, 0x00, 0, 0x80, 0x80, 0x80])
    );
    assert_eq!(
        run(MmxIntKind::Paddsb, d, s, 0),
        bytes([0x7f, 0x80, 0x00, 0x00, 0, 0x7f, 0x80, 0x7f])
    );
    assert_eq!(
        run(MmxIntKind::Paddusb, d, s, 0),
        bytes([0x80, 0xff, 0xff, 0xff, 0, 0x80, 0xff, 0x80])
    );
    // PSUBUSB clamps at 0, PSUBSB at -128/127.
    assert_eq!(
        run(MmxIntKind::Psubusb, d, s, 0),
        bytes([0x7e, 0x00, 0xfe, 0x00, 0, 0, 0, 0x7c])
    );
    assert_eq!(
        run(MmxIntKind::Psubsb, d, s, 0),
        bytes([0x7e, 0x81, 0xfe, 0x02, 0, 0, 0, 0x7c])
    );
    // PMULLW / PMULHW: 0x8000 * 0x8000 = 0x4000_0000, -2 * 3 = -6.
    let d = words([0x8000, 0xfffe, 0x7fff, 0x0100]);
    let s = words([0x8000, 0x0003, 0x7fff, 0x0100]);
    assert_eq!(
        run(MmxIntKind::Pmullw, d, s, 0),
        words([0x0000, 0xfffa, 0x0001, 0x0000])
    );
    assert_eq!(
        run(MmxIntKind::Pmulhw, d, s, 0),
        words([0x4000, 0xffff, 0x3fff, 0x0001])
    );
    // PMADDWD: the only overflowing sum, 0x8000*0x8000 twice, wraps to 0x8000_0000.
    assert_eq!(
        run(
            MmxIntKind::Pmaddwd,
            words([0x8000, 0x8000, 1, 0xffff]),
            words([0x8000, 0x8000, 2, 3]),
            0
        ),
        dwords([0x8000_0000, 0xffff_ffff])
    );
    // PCMPGTB is signed: 0x80 (-128) is not greater than 0x7f.
    assert_eq!(
        run(
            MmxIntKind::Pcmpgtb,
            bytes([0x7f, 0x80, 1, 2, 3, 4, 5, 6]),
            bytes([0x80, 0x7f, 1, 1, 4, 3, 5, 5]),
            0
        ),
        bytes([0xff, 0, 0, 0xff, 0, 0xff, 0, 0xff])
    );
    // PUNPCK: interleave the low (high) halves, destination first.
    let d = bytes([0, 1, 2, 3, 4, 5, 6, 7]);
    let s = bytes([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
    assert_eq!(
        run(MmxIntKind::Punpcklbw, d, s, 0),
        bytes([0, 0x10, 1, 0x11, 2, 0x12, 3, 0x13])
    );
    assert_eq!(
        run(MmxIntKind::Punpckhbw, d, s, 0),
        bytes([4, 0x14, 5, 0x15, 6, 0x16, 7, 0x17])
    );
    assert_eq!(
        run(MmxIntKind::Punpckldq, d, s, 0),
        bytes([0, 1, 2, 3, 0x10, 0x11, 0x12, 0x13])
    );
    assert_eq!(
        run(MmxIntKind::Punpckhdq, d, s, 0),
        bytes([4, 5, 6, 7, 0x14, 0x15, 0x16, 0x17])
    );
    // PACK: the destination words fill the low half of the result, the source words the high half.
    let d = words([0x0080, 0xff7f, 0x7fff, 0x8000]);
    let s = words([0x0100, 0xffff, 0x007f, 0x0000]);
    assert_eq!(
        run(MmxIntKind::Packsswb, d, s, 0),
        bytes([0x7f, 0x80, 0x7f, 0x80, 0x7f, 0xff, 0x7f, 0x00])
    );
    assert_eq!(
        run(MmxIntKind::Packuswb, d, s, 0),
        bytes([0x80, 0x00, 0xff, 0x00, 0xff, 0x00, 0x7f, 0x00])
    );
    assert_eq!(
        run(
            MmxIntKind::Packssdw,
            dwords([0x0001_8000, 0xffff_7fff]),
            dwords([0x0000_7fff, 0x8000_0000]),
            0
        ),
        words([0x7fff, 0x8000, 0x7fff, 0x8000])
    );
    // PSHUFW: 0x1b reverses the four words.
    assert_eq!(
        run(
            MmxIntKind::Pshufw,
            0,
            words([0x1111, 0x2222, 0x3333, 0x4444]),
            0x1b
        ),
        words([0x4444, 0x3333, 0x2222, 0x1111])
    );
}

// ---- execution on the x87 registers ----

fn code_reg(opcode: u8, imm: Option<u8>) -> Vec<u8> {
    let mut code = vec![0x0f, opcode, 0xc1]; // dest MM0, source MM1
    code.extend(imm);
    code
}

fn code_mem(opcode: u8, imm: Option<u8>) -> Vec<u8> {
    let mut code = vec![0x0f, opcode, 0x00]; // dest MM0, source [rax]
    code.extend(imm);
    code
}

fn exec(
    code: &[u8],
    mut cpu: LongModeState,
) -> (Result<StepOutcome, MemoryError>, LongModeState, Ram) {
    let mut ram = Ram::new(code);
    let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
    (outcome, cpu, ram)
}

/// Whole-state check of a retired MMX instruction that wrote MM[index]: TOP = 0, every tag valid, bits 79:64 of
/// the written register all ones, nothing else of the x87, GPR, flag or XMM state changed.
fn assert_wrote_only(
    before: &LongModeState,
    after: &LongModeState,
    written: Option<usize>,
    what: &str,
) {
    let (b, a) = (before.x87.as_ref().unwrap(), after.x87.as_ref().unwrap());
    assert_eq!(a.status_word, b.status_word & !(7 << 11), "{what}: TOP");
    assert_eq!(a.tag_word, 0, "{what}: tags");
    assert_eq!(a.control_word, b.control_word, "{what}: FCW");
    assert_eq!(a.instruction_pointer, b.instruction_pointer, "{what}: FIP");
    assert_eq!(a.data_pointer, b.data_pointer, "{what}: FDP");
    assert_eq!(a.last_opcode, b.last_opcode, "{what}: FOP");
    for index in 0..8 {
        if Some(index) == written {
            assert_eq!(
                a.registers[index][8..],
                [0xff, 0xff],
                "{what}: exponent of MM{index}"
            );
        } else {
            assert_eq!(
                a.registers[index], b.registers[index],
                "{what}: MM{index} rewritten"
            );
        }
    }
    assert_eq!(after.sse, before.sse, "{what}: SSE state");
    assert_eq!(after.rflags, before.rflags, "{what}: RFLAGS");
}

#[test]
fn every_two_operand_form_writes_only_the_destination_register_and_the_mmx_state_effect() {
    let mut rng = Rng(0x39_01);
    for (name, opcode, kind) in KINDS {
        for round in 0..24usize {
            let (dest, src) = (rng.vector(round), rng.vector(round / 2 + 1));
            let imm = if *name == "pshufw" {
                Some(rng.next() as u8)
            } else {
                None
            };
            for memory in [false, true] {
                let mut cpu = cpu();
                set_mm(&mut cpu, 0, dest);
                set_mm(&mut cpu, 1, if memory { rng.next() } else { src });
                let code = if memory {
                    code_mem(*opcode, imm)
                } else {
                    code_reg(*opcode, imm)
                };
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                ram.put(DATA, &src.to_le_bytes());
                let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
                assert_eq!(cpu.rip, CODE + code.len() as u64, "{name}: RIP");
                let expected = u64::from_le_bytes(mmx_int_result(
                    *kind,
                    dest.to_le_bytes(),
                    src.to_le_bytes(),
                    imm.unwrap_or(0),
                ));
                assert_eq!(mm(&cpu, 0), expected, "{name} memory={memory}");
                assert_eq!(
                    mm(&cpu, 0),
                    reference(name, dest, src, imm.unwrap_or(0)),
                    "{name}"
                );
                assert_wrote_only(&before, &cpu, Some(0), name);
                assert_eq!(cpu.gpr, before.gpr, "{name}: GPR");
                if memory {
                    assert_eq!(ram.reads, vec![(DATA, 8)], "{name}: one 8-byte read");
                    assert!(ram.writes.is_empty(), "{name}: no write");
                }
            }
        }
    }
}

#[test]
fn every_destination_and_source_register_pair_changes_only_the_destination() {
    for destination in 0..8u8 {
        for source in 0..8u8 {
            let mut cpu = cpu();
            let (d, s) = (mm(&cpu, destination.into()), mm(&cpu, source.into()));
            let code = [0x0f, 0xfd, 0xc0 | destination << 3 | source];
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(mm(&cpu, destination.into()), reference("paddw", d, s, 0));
            assert_wrote_only(&before, &cpu, Some(destination.into()), "paddw pair");
        }
    }
}

#[test]
fn a_source_register_that_is_also_the_destination_reads_the_old_value() {
    // PXOR mm3, mm3 clears the register; PSUBB mm2, mm2 likewise; PADDW mm4, mm4 doubles every word.
    for (opcode, expected) in [
        (0xefu8, 0u64),
        (0xf8, 0),
        (
            0xfd,
            reference("paddw", 0x1111_2222_3333_4444, 0x1111_2222_3333_4444, 0),
        ),
    ] {
        let mut cpu = cpu();
        set_mm(&mut cpu, 3, 0x1111_2222_3333_4444);
        let (outcome, cpu, _) = exec(&[0x0f, opcode, 0xdb], cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        assert_eq!(mm(&cpu, 3), expected);
        assert_eq!(
            exponent(&cpu, 3),
            0xffff,
            "the destination exponent is set even by PXOR"
        );
    }
}

#[test]
fn shifts_by_register_and_by_immediate_execute_every_kind_and_count() {
    let groups: [(u8, u8, &str); 8] = [
        (0x71, 2, "psrlw"),
        (0x71, 4, "psraw"),
        (0x71, 6, "psllw"),
        (0x72, 2, "psrld"),
        (0x72, 4, "psrad"),
        (0x72, 6, "pslld"),
        (0x73, 2, "psrlq"),
        (0x73, 6, "psllq"),
    ];
    let value = 0x8421_c3a5_ff00_7ffeu64;
    for (opcode, reg, name) in groups {
        for count in (0..=70u8).chain([127, 128, 129, 191, 192, 255]) {
            // destination MM5, the count in the imm8
            let mut cpu = cpu();
            set_mm(&mut cpu, 5, value);
            let before = cpu.clone();
            let code = [0x0f, opcode, 0xc0 | reg << 3 | 5, count];
            let (outcome, cpu, _) = exec(&code, cpu);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name} {count}");
            assert_eq!(cpu.rip, CODE + 4);
            assert_eq!(
                mm(&cpu, 5),
                reference(name, value, u64::from(count), 0),
                "{name} imm {count}"
            );
            assert_wrote_only(&before, &cpu, Some(5), name);
        }
    }
}

#[test]
fn shift_group_encodings_without_an_mmx_form_are_undefined_opcodes() {
    // /0, /1, /3, /5, /7 of 0F 71/72 and /0, /1, /4, /5, /3, /7 of 0F 73 (no MMX form), and every memory form.
    for opcode in [0x71u8, 0x72, 0x73] {
        for reg in 0..8u8 {
            let valid = match opcode {
                0x73 => matches!(reg, 2 | 6),
                _ => matches!(reg, 2 | 4 | 6),
            };
            for modrm in [0xc0 | reg << 3 | 1, reg << 3] {
                let register_form = modrm >= 0xc0;
                let (outcome, after, _) = exec(&[0x0f, opcode, modrm, 3], cpu());
                if valid && register_form {
                    assert_eq!(outcome, Ok(StepOutcome::Advanced), "{opcode:02x} /{reg}");
                } else {
                    assert_eq!(outcome, Ok(ud()), "{opcode:02x} /{reg} modrm {modrm:02x}");
                    assert_eq!(after.x87, cpu().x87, "no state change");
                    assert_eq!(after.rip, CODE);
                }
            }
        }
    }
}

#[test]
fn pshufw_selects_words_for_every_immediate_from_register_and_memory() {
    for imm in 0..=255u8 {
        let src = 0x4444_3333_2222_1111u64;
        let mut cpu = cpu();
        set_mm(&mut cpu, 1, src);
        set_mm(&mut cpu, 0, 0xdead_beef_dead_beef);
        let (outcome, after, _) = exec(&[0x0f, 0x70, 0xc1, imm], cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        assert_eq!(
            mm(&after, 0),
            reference("pshufw", 0, src, imm),
            "imm {imm:#x}"
        );
        let mut ram = Ram::new(&[0x0f, 0x70, 0x00, imm]);
        ram.put(DATA, &src.to_le_bytes());
        let mut cpu = self::cpu();
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            mm(&cpu, 0),
            reference("pshufw", 0, src, imm),
            "memory imm {imm:#x}"
        );
    }
}

// ---- MOVD / MOVQ ----

#[test]
fn movd_and_movq_load_a_general_register_or_memory_into_the_zero_extended_mmx_register() {
    let rax = 0xfedc_ba98_7654_3210u64;
    // MOVD mm1, eax: the low doubleword, zero-extended. REX.W: MOVQ mm1, rax.
    for (code, expected) in [
        (vec![0x0f, 0x6e, 0xc8], 0x7654_3210u64),
        (vec![0x48, 0x0f, 0x6e, 0xc8], rax),
        (vec![0x44, 0x0f, 0x6e, 0xc8], 0x7654_3210), // REX.R does not reach MM1
    ] {
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = rax;
        let before = cpu.clone();
        let (outcome, after, _) = exec(&code, cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        assert_eq!(mm(&after, 1), expected, "{code:02x?}");
        assert_wrote_only(&before, &after, Some(1), "movd load");
        assert_eq!(after.gpr, before.gpr);
    }
    // r9d with REX.B: MOVD mm1, r9d (41 0F 6E C9).
    let mut cpu = cpu();
    cpu.gpr[9] = 0x1_8000_0001;
    let (outcome, after, _) = exec(&[0x41, 0x0f, 0x6e, 0xc9], cpu);
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    assert_eq!(mm(&after, 1), 0x8000_0001);
    // From memory: 4 bytes (MOVD) or 8 bytes (MOVQ), little endian.
    let mut ram = Ram::new(&[0x0f, 0x6e, 0x08]);
    ram.put(DATA, &[1, 2, 3, 4, 5, 6, 7, 8]);
    let mut cpu = self::cpu();
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(mm(&cpu, 1), 0x0403_0201);
    assert_eq!(ram.reads, vec![(DATA, 4)]);
    let mut ram = Ram::new(&[0x48, 0x0f, 0x6e, 0x08]);
    ram.put(DATA, &[1, 2, 3, 4, 5, 6, 7, 8]);
    let mut cpu = self::cpu();
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(mm(&cpu, 1), 0x0807_0605_0403_0201);
    assert_eq!(ram.reads, vec![(DATA, 8)]);
}

#[test]
fn movd_and_movq_store_the_low_part_of_the_mmx_register_and_read_it_with_the_same_state_effect() {
    let value = 0x0123_4567_89ab_cdefu64;
    // MOVD eax, mm1 zero-extends into RAX (upper half cleared); MOVQ rax, mm1 takes all 64 bits.
    for (code, expected) in [
        (vec![0x0f, 0x7e, 0xc8], 0x89ab_cdefu64),
        (vec![0x48, 0x0f, 0x7e, 0xc8], value),
    ] {
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = u64::MAX;
        set_mm(&mut cpu, 1, value);
        let before = cpu.clone();
        let (outcome, after, _) = exec(&code, cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        assert_eq!(after.gpr[Register::Rax as usize], expected, "{code:02x?}");
        // The MMX register is only read: not rewritten, the exponent stays, but TOP and the tags change.
        assert_wrote_only(&before, &after, None, "movd store to a register");
    }
    // To memory: 4 or 8 bytes, the neighbours keep the sentinel.
    for (code, length) in [
        (vec![0x0f, 0x7e, 0x08], 4usize),
        (vec![0x48, 0x0f, 0x7e, 0x08], 8),
    ] {
        let mut cpu = cpu();
        set_mm(&mut cpu, 1, value);
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(ram.get(DATA, length), &value.to_le_bytes()[..length]);
        assert!(
            ram.get(DATA + length as u64, 8)
                .iter()
                .all(|byte| *byte == SENTINEL)
        );
        assert_eq!(ram.writes, vec![(DATA, length)]);
        assert_wrote_only(&before, &cpu, None, "movd store to memory");
    }
}

#[test]
fn movq_moves_between_mmx_registers_and_memory_in_both_directions() {
    let mut cpu = cpu();
    set_mm(&mut cpu, 2, 0x1122_3344_5566_7788);
    let before = cpu.clone();
    // MOVQ mm1, mm2 (0F 6F CA) and MOVQ mm1, mm2 by the store encoding (0F 7F D1).
    for code in [[0x0f, 0x6f, 0xca], [0x0f, 0x7f, 0xd1]] {
        let (outcome, after, _) = exec(&code, cpu.clone());
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        assert_eq!(mm(&after, 1), 0x1122_3344_5566_7788);
        assert_wrote_only(&before, &after, Some(1), "movq register");
    }
    // Load from and store to memory.
    let mut ram = Ram::new(&[0x0f, 0x6f, 0x08]);
    ram.put(DATA, &0x8070_6050_4030_2010u64.to_le_bytes());
    let mut load = self::cpu();
    assert_eq!(
        step(&mut load, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(mm(&load, 1), 0x8070_6050_4030_2010u64);
    let mut ram = Ram::new(&[0x0f, 0x7f, 0x10]);
    let mut store = self::cpu();
    set_mm(&mut store, 2, 0x0807_0605_0403_0201);
    let store_before = store.clone();
    assert_eq!(
        step(&mut store, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(ram.get(DATA, 8), &0x0807_0605_0403_0201u64.to_le_bytes());
    assert_eq!(ram.writes, vec![(DATA, 8)]);
    assert_wrote_only(&store_before, &store, None, "movq store");
}

#[test]
fn the_memory_forms_need_no_alignment_and_a_failing_access_changes_nothing() {
    for offset in 0..8u64 {
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = DATA + offset;
        let mut ram = Ram::new(&[0x0f, 0xfd, 0x00]);
        for byte in 0..16 {
            ram.put(DATA + byte, &[byte as u8 + 1]);
        }
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced)
        );
        let source = u64::from_le_bytes(ram.get(DATA + offset, 8).try_into().unwrap());
        assert_eq!(
            mm(&cpu, 0),
            reference("paddw", mm(&self::cpu(), 0), source, 0)
        );
    }
    // A backend fault on a load or a store: the CPU state (x87 included) and the memory are untouched.
    for code in [
        vec![0x0f, 0xfd, 0x00],
        vec![0x0f, 0x6f, 0x00],
        vec![0x0f, 0x6e, 0x00],
        vec![0x0f, 0x7f, 0x00],
        vec![0x0f, 0x7e, 0x00],
    ] {
        let mut cpu = cpu();
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(DATA);
        let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert!(
            matches!(&outcome, Ok(StepOutcome::Fault(fault)) if fault.vector == 14),
            "{code:02x?}: {outcome:?}"
        );
        assert_eq!(cpu, before, "{code:02x?}: the state changed");
        assert!(ram.get(DATA, 8).iter().all(|byte| *byte == SENTINEL));
    }
    // A non-canonical address is #GP(0) before any memory access (#SS(0) for an RSP-based operand).
    for (code, expected) in [
        (vec![0x0f, 0xfd, 0x00], gp0()),
        (vec![0x0f, 0x7f, 0x00], gp0()),
        (vec![0x0f, 0x6e, 0x00], gp0()),
        (vec![0x0f, 0x7e, 0x00], gp0()),
        (vec![0x0f, 0xfd, 0x04, 0x24], ss0()),
    ] {
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
        cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(expected),
            "{code:02x?}"
        );
        assert_eq!(cpu, before);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }
}

#[test]
fn at_cpl3_with_the_alignment_check_armed_a_misaligned_operand_is_alignment_check_and_an_aligned_one_runs()
 {
    for (code, size) in [
        (vec![0x0f, 0xfd, 0x00], 8u64),
        (vec![0x0f, 0x6f, 0x00], 8),
        (vec![0x0f, 0x7f, 0x00], 8),
        (vec![0x0f, 0x6e, 0x00], 4),
        (vec![0x0f, 0x7e, 0x00], 4),
        (vec![0x48, 0x0f, 0x6e, 0x00], 8),
    ] {
        for offset in [0u64, 1, 2, 4, 8] {
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = DATA + offset;
            cpu.rflags |= 1 << 18;
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring3);
            if offset % size == 0 {
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{code:02x?} +{offset}");
            } else {
                // ADR-0023 C1 (decision 4): #AC(0) by the size of the MMX operand (it was a refusal).
                assert_eq!(
                    outcome,
                    Ok(StepOutcome::Fault(od_isa::ArchitecturalFault {
                        vector: 17,
                        error_code: Some(0),
                        linear_address: None
                    })),
                    "{code:02x?} +{offset}"
                );
                assert_eq!(cpu, before);
                assert!(ram.writes.is_empty());
            }
        }
    }
}

// ---- gates and the x87 state ----

fn gated(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    prepare: impl FnOnce(&mut LongModeState),
) -> (RunExit, LongModeState, LongModeState) {
    let mut cpu = fp_init_cpu(cr0, cr4);
    prepare(&mut cpu);
    let before = cpu.clone();
    let mut ram = Ram::new(code);
    ram.put(DATA, &[0x55; 8]);
    let (result, after) = run_fp_init(cpu, &mut ram);
    (result.exit, before, after)
}

fn all_mmx_codes() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("paddb mm0,mm1", vec![0x0f, 0xfc, 0xc1]),
        ("paddw mm0,[rax]", vec![0x0f, 0xfd, 0x00]),
        ("pcmpgtd mm2,mm3", vec![0x0f, 0x66, 0xd3]),
        ("pshufw mm0,mm1,0x1b", vec![0x0f, 0x70, 0xc1, 0x1b]),
        ("psllq mm3,4", vec![0x0f, 0x73, 0xf3, 0x04]),
        ("psraw mm3,mm2", vec![0x0f, 0xe1, 0xda]),
        ("movd mm1,eax", vec![0x0f, 0x6e, 0xc8]),
        ("movd eax,mm1", vec![0x0f, 0x7e, 0xc8]),
        ("movd [rax],mm1", vec![0x0f, 0x7e, 0x08]),
        ("movq mm1,[rax]", vec![0x0f, 0x6f, 0x08]),
        ("movq [rax],mm1", vec![0x0f, 0x7f, 0x08]),
        ("movq mm1,mm2", vec![0x0f, 0x6f, 0xca]),
    ]
}

#[test]
fn cr0_em_is_ud_cr0_ts_is_nm_and_a_pending_x87_exception_is_mf_before_any_state_change() {
    for (name, code) in all_mmx_codes() {
        let ok = gated(&code, CR0_BASE, CR4_PAE, |_| {});
        assert_eq!(ok.0, RunExit::InstructionBudgetExhausted, "{name}: ungated");
        // CR4.OSFXSR does not gate an MMX form (Vol. 2A Table 2-21 lists it for legacy SSE only).
        let osfxsr = gated(&code, CR0_BASE, CR4_PAE | CR4_OSFXSR, |_| {});
        assert_eq!(
            osfxsr.0,
            RunExit::InstructionBudgetExhausted,
            "{name}: OSFXSR"
        );
        for (cr0, vector) in [
            (CR0_BASE | 1 << 2, 6u8),
            (CR0_BASE | 1 << 3, 7),
            (CR0_BASE | 1 << 2 | 1 << 3, 6),
        ] {
            let (exit, before, after) = gated(&code, cr0, CR4_PAE | CR4_OSFXSR, |_| {});
            let RunExit::Fault(fault) = exit else {
                panic!("{name}: {exit:?}")
            };
            assert_eq!(fault.vector, vector, "{name} CR0 {cr0:#x}");
            assert_eq!(
                after, before,
                "{name}: the faulting instruction changed state"
            );
        }
        // IE pending and unmasked (FCW 0x037e): #MF, state unchanged.
        let (exit, before, after) = gated(&code, CR0_BASE, CR4_PAE | CR4_OSFXSR, |cpu| {
            let x87 = cpu.x87.as_mut().unwrap();
            x87.control_word = 0x037e;
            x87.status_word = 0x0081 | 5 << 11;
        });
        let RunExit::Fault(fault) = exit else {
            panic!("{name}: {exit:?}")
        };
        assert_eq!(fault.vector, 16, "{name}");
        assert_eq!(after, before, "{name}: #MF changed state");
        // A masked pending flag raises nothing.
        let (exit, ..) = gated(&code, CR0_BASE, CR4_PAE | CR4_OSFXSR, |cpu| {
            let x87 = cpu.x87.as_mut().unwrap();
            x87.control_word = 0x037f;
            x87.status_word = 0x0081;
        });
        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{name}: masked");
    }
}

#[test]
fn an_x87_program_state_becomes_mmx_state_and_emms_empties_it() {
    // x87 seed: TOP = 5, tags 0xa5a5. After any MMX instruction TOP = 0 and all tags valid; after EMMS all empty,
    // TOP = 0, the registers and other state untouched.
    for (name, code) in all_mmx_codes() {
        let before = cpu();
        let (outcome, after, _) = exec(&code, before.clone());
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
        let x87 = after.x87.as_ref().unwrap();
        assert_eq!(x87.status_word & (7 << 11), 0, "{name}: TOP");
        assert_eq!(x87.tag_word, 0, "{name}: tags valid");
        assert_eq!(
            x87.status_word & !(7 << 11),
            before.x87.as_ref().unwrap().status_word & !(7 << 11)
        );
        let mut resumed = after.clone();
        resumed.rip = CODE;
        let (outcome, emptied, _) = exec(&[0x0f, 0x77], resumed);
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        let e = emptied.x87.as_ref().unwrap();
        assert_eq!(e.tag_word, 0xffff);
        assert_eq!(e.status_word & (7 << 11), 0);
        assert_eq!(e.registers, x87.registers);
    }
}

#[test]
fn lock_is_ud_for_every_form_and_unclaimed_prefixes_stop_as_unsupported_without_change() {
    for (name, code) in all_mmx_codes() {
        let mut locked = vec![0xf0];
        locked.extend(&code);
        let (outcome, after, ram) = exec(&locked, cpu());
        assert_eq!(outcome, Ok(ud()), "{name}: LOCK");
        assert_eq!(after, cpu(), "{name}: LOCK changed state");
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
        for prefix in [
            vec![0xf2u8],
            vec![0xf3],
            vec![0x64],
            vec![0x65],
            vec![0x67, 0x67],
            vec![0x2e, 0x3e],
        ] {
            // F2/F3 with the memory forms of 0F 6F/7F are MOVDQU, not an MMX form; its register forms are executed since
            // S2-39 (integer SSE2 remainder).
            if prefix[0] >= 0xf2 && matches!(code[..2], [0x0f, 0x6f | 0x7f]) && code[2] < 0xc0 {
                continue;
            }
            if prefix[0] == 0xf3 && matches!(code[..2], [0x0f, 0x6f | 0x7f]) {
                continue;
            }
            // F2/F3 with 0F 70 are PSHUFLW/PSHUFHW (S2-39, integer SSE2 remainder), not PSHUFW.
            if prefix[0] >= 0xf2 && matches!(code[..2], [0x0f, 0x70]) {
                continue;
            }
            // F3 0F 7E is MOVQ xmm, xmm/m64 (the same slice).
            if prefix[0] == 0xf3 && matches!(code[..2], [0x0f, 0x7e]) {
                continue;
            }
            let mut prefixed = prefix.clone();
            prefixed.extend(&code);
            let (outcome, after, _) = exec(&prefixed, cpu());
            assert!(
                matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
                "{name} with {prefix:02x?}: {outcome:?}"
            );
            assert_eq!(after, cpu(), "{name} with {prefix:02x?}: state changed");
        }
        // REX.W, the 67 and null segment overrides reach no MMX register operand and are accepted for the
        // register forms; the length and the result are those of the plain form for the non-MOVD forms.
    }
    // The 128-bit form (66 prefix) is not an MMX form: without the SSE state of the narrow profile it never
    // reaches an MMX register.
    let (outcome, after, _) = exec(&[0x66, 0x0f, 0xfc, 0xc1], cpu());
    let x87 = after.x87.as_ref().unwrap();
    assert_eq!(
        x87.tag_word,
        cpu().x87.as_ref().unwrap().tag_word,
        "the 66 form leaves the x87 state alone ({outcome:?})"
    );
}

#[test]
fn rex_and_address_overrides_do_not_change_the_register_numbers_of_the_mmx_forms() {
    let reference_cpu = {
        let (_, after, _) = exec(&[0x0f, 0xfd, 0xd1], cpu());
        after
    };
    for prefix in [
        vec![0x48u8],
        vec![0x44],
        vec![0x41],
        vec![0x42],
        vec![0x4d],
        vec![0x67],
        vec![0x2e],
        vec![0x3e],
        vec![0x26],
        vec![0x36],
    ] {
        let mut code = prefix.clone();
        code.extend([0x0f, 0xfd, 0xd1]);
        let (outcome, after, _) = exec(&code, cpu());
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{prefix:02x?}");
        assert_eq!(after.x87, reference_cpu.x87, "{prefix:02x?}");
        assert_eq!(after.rip, CODE + code.len() as u64);
    }
}

// ---- the decoder ----

fn decode(code: &[u8]) -> Result<long_decode::Instruction, DecodeError> {
    long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default())
}

#[test]
fn the_decoder_returns_the_mmx_operations_with_their_register_numbers_and_lengths() {
    let i = decode(&[0x0f, 0xfd, 0xd1]).unwrap();
    assert_eq!(i.length, 3);
    assert!(matches!(
        i.operation,
        Operation::MmxInt {
            kind: MmxIntKind::Paddw,
            destination: 2,
            source: Sse3Source::Register(1),
            immediate: 0
        }
    ));
    // REX.R and REX.B do not extend MM0..MM7.
    let i = decode(&[0x4d, 0x0f, 0xfd, 0xd1]).unwrap();
    assert!(matches!(
        i.operation,
        Operation::MmxInt {
            kind: MmxIntKind::Paddw,
            destination: 2,
            source: Sse3Source::Register(1),
            ..
        }
    ));
    // A memory source with SIB and displacement; REX.B/REX.X belong to the address.
    let i = decode(&[0x43, 0x0f, 0xdb, 0x44, 0x88, 0x10]).unwrap();
    assert_eq!(i.length, 6);
    let Operation::MmxInt {
        kind: MmxIntKind::Pand,
        destination: 0,
        source: Sse3Source::Memory(memory),
        ..
    } = i.operation
    else {
        panic!("{:?}", i.operation)
    };
    assert_eq!(memory.displacement, 0x10);
    let i = decode(&[0x0f, 0x70, 0xd1, 0x1b]).unwrap();
    assert_eq!(i.length, 4);
    assert!(matches!(
        i.operation,
        Operation::MmxInt {
            kind: MmxIntKind::Pshufw,
            destination: 2,
            source: Sse3Source::Register(1),
            immediate: 0x1b
        }
    ));
    let i = decode(&[0x0f, 0x71, 0xf5, 0x07]).unwrap();
    assert!(matches!(
        i.operation,
        Operation::MmxShiftImmediate {
            kind: MmxIntKind::Psllw,
            destination: 5,
            immediate: 7
        }
    ));
    let i = decode(&[0x41, 0x0f, 0x73, 0xd7, 0x40]).unwrap();
    assert!(matches!(
        i.operation,
        Operation::MmxShiftImmediate {
            kind: MmxIntKind::Psrlq,
            destination: 7,
            immediate: 0x40
        }
    ));
    // MOVD/MOVQ: the general register is extended by REX.B, the size by REX.W.
    let i = decode(&[0x49, 0x0f, 0x6e, 0xd9]).unwrap();
    let Operation::MmxMovd {
        to_mmx: true,
        mmx: 3,
        operand: Operand::Register(view),
    } = i.operation
    else {
        panic!("{:?}", i.operation)
    };
    assert_eq!(
        view,
        od_isa::RegisterView::low(Register::R9, od_isa::OperandSize::Qword)
    );
    let i = decode(&[0x0f, 0x7e, 0xd9]).unwrap();
    assert!(matches!(
        i.operation,
        Operation::MmxMovd {
            to_mmx: false,
            mmx: 3,
            operand: Operand::Register(_)
        }
    ));
    let i = decode(&[0x0f, 0x6f, 0xd9]).unwrap();
    assert!(matches!(
        i.operation,
        Operation::MmxMovq {
            to_mmx: true,
            mmx: 3,
            other: Sse3Source::Register(1)
        }
    ));
    let i = decode(&[0x0f, 0x7f, 0x18]).unwrap();
    assert!(matches!(
        i.operation,
        Operation::MmxMovq {
            to_mmx: false,
            mmx: 3,
            other: Sse3Source::Memory(_)
        }
    ));
}

#[test]
fn the_decoder_leaves_the_sse_forms_of_the_same_opcodes_alone_and_refuses_the_prefixes_it_does_not_model()
 {
    // 66 0F 6F / F3 0F 6F stay the 128-bit moves; F2/F3/FS/GS/duplicate prefixes on an MMX form are unclaimed.
    for code in [
        &[0xf3u8, 0x0f, 0x6f, 0x00][..],
        &[0x66, 0x0f, 0x7f, 0x00],
        &[0x66, 0x0f, 0x6f, 0x00],
    ] {
        let operation = decode(code).unwrap().operation;
        assert!(
            !matches!(operation, Operation::MmxMovq { .. }),
            "{code:02x?}: {operation:?}"
        );
    }
    for code in [
        &[0xf2u8, 0x0f, 0xfd, 0xc1][..],
        &[0xf3, 0x0f, 0xfd, 0xc1],
        &[0x64, 0x0f, 0xfd, 0x00],
        &[0x65, 0x0f, 0xfd, 0x00],
        &[0x67, 0x67, 0x0f, 0xfd, 0x00],
        &[0x40, 0x40, 0x0f, 0xfd, 0xc1],
        &[0xf3, 0x0f, 0x6e, 0xc1],
        &[0xf2, 0x0f, 0x7f, 0xc1],
    ] {
        assert!(
            matches!(decode(code), Err(DecodeError::Unsupported { .. })),
            "{code:02x?}: {:?}",
            decode(code)
        );
    }
    for code in [
        &[0xf0u8, 0x0f, 0xfd, 0xc1][..],
        &[0xf0, 0x0f, 0x6e, 0x00],
        &[0xf0, 0x0f, 0x6f, 0xc1],
        &[0xf0, 0x0f, 0x71, 0xd1, 1],
    ] {
        assert!(
            matches!(decode(code), Err(DecodeError::Invalid { .. })),
            "{code:02x?}: {:?}",
            decode(code)
        );
    }
}
