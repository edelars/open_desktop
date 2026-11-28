//! S2-54 (MMX forms): the integer SSSE3 instructions on MMX registers, without the mandatory `66` prefix:
//! PSHUFB, PALIGNR, PABSB/PABSW/PABSD, PSIGNB/PSIGNW/PSIGND, PMADDUBSW, PMULHRSW, PHADDW/PHADDD, PHSUBW/PHSUBD,
//! PHADDSW and PHSUBSW (`0F 38 00..0B/1C..1E /r`, `0F 3A 0F /r ib`).
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! cdrdv2.intel.com/v1/dl/getContent/671200, through pdftotext): the "Operation" sections "(With 64-bit
//! Operands)" of PSHUFB pp. 4-421..4-425, PALIGNR pp. 4-216..4-219, PABSB/PABSW/PABSD pp. 4-177..4-182,
//! PSIGNB/PSIGNW/PSIGND pp. 4-437..4-440, PMADDUBSW pp. 4-310..4-312, PMULHRSW pp. 4-375..4-378,
//! PHADDW/PHADDD pp. 4-294..4-297, PHSUBW/PHSUBD pp. 4-302..4-304, PHADDSW pp. 4-292..4-293, PHSUBSW
//! pp. 4-300..4-301; Vol. 2A Table 2-21 (Exceptions Type 4, p. 2-29: the 16-byte alignment #GP is for a
//! 128-bit operand, a 64-bit MMX operand has none); Vol. 1 §9.6.2 and Vol. 3A §15.2 (the effect on the
//! x87 state, tested in `s2_33_mmx.rs`). The vector tests carry values worked out by hand from the
//! pseudocode; the matrix tests compare against `reference64`, a second, literal transcription of that
//! pseudocode on 64-bit integers that shares no code with the model.
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the CPUID
//! gate (S2-57), `#AC` for a misaligned 8-byte operand at CPL3 (the model stops with `Unsupported`), the
//! behaviour of `F2`/`F3`, duplicated prefixes and FS/GS overrides (refused, not guessed).

mod mmx_common;

use mmx_common::*;
use od_isa::execute::StepOutcome;
use od_isa::long_decode::{self, Operation, Sse3Source, Ssse3IntKind};
use od_isa::{AccessContext, LongModeState, PrivilegeLevel, Register};

// ---- the reference model: the SDM pseudocode (64-bit operands), transcribed literally ----

fn sbyte(value: u64, index: usize) -> i64 {
    i64::from((value >> (8 * index)) as u8 as i8)
}
fn ubyte(value: u64, index: usize) -> i64 {
    i64::from((value >> (8 * index)) as u8)
}
fn sword(value: u64, index: usize) -> i64 {
    i64::from((value >> (16 * index)) as u16 as i16)
}
fn sdword(value: u64, index: usize) -> i64 {
    i64::from((value >> (32 * index)) as u32 as i32)
}
fn pack(width_bits: usize, lanes: &[i64]) -> u64 {
    let mask = (1u64 << width_bits) - 1;
    lanes.iter().enumerate().fold(0, |acc, (index, lane)| {
        acc | ((*lane as u64 & mask) << (width_bits * index))
    })
}
fn saturate_to_signed_word(value: i64) -> i64 {
    value.clamp(-32768, 32767)
}

/// `dest` is the destination register (mm1), `src` the source (mm2/m64).
fn reference64(kind: Ssse3IntKind, dest: u64, src: u64, imm8: u8) -> u64 {
    match kind {
        Ssse3IntKind::Pshufb => {
            // PSHUFB (with 64-bit MMX operands): TEMP := DEST; srcpos := shufbyte & 07H
            let temp = dest;
            let lanes: Vec<i64> = (0..8)
                .map(|destpos| {
                    let shufbyte = ubyte(src, destpos);
                    if shufbyte & 0x80 == 0x80 {
                        0
                    } else {
                        ubyte(temp, (shufbyte & 0x07) as usize)
                    }
                })
                .collect();
            pack(8, &lanes)
        }
        Ssse3IntKind::Palignr => {
            // temp1[127:0] = CONCATENATE(DEST,SRC) >> (imm8*8); DEST[63:0] = temp1[63:0]
            let concatenated = (u128::from(dest) << 64) | u128::from(src);
            let shift = u32::from(imm8) * 8;
            let temp1 = if shift >= 128 {
                0
            } else {
                concatenated >> shift
            };
            temp1 as u64
        }
        Ssse3IntKind::Pabsb => pack(8, &(0..8).map(|i| sbyte(src, i).abs()).collect::<Vec<_>>()),
        Ssse3IntKind::Pabsw => pack(16, &(0..4).map(|i| sword(src, i).abs()).collect::<Vec<_>>()),
        Ssse3IntKind::Pabsd => pack(
            32,
            &(0..2).map(|i| sdword(src, i).abs()).collect::<Vec<_>>(),
        ),
        Ssse3IntKind::Psignb | Ssse3IntKind::Psignw | Ssse3IntKind::Psignd => {
            let (width, count) = match kind {
                Ssse3IntKind::Psignb => (8, 8),
                Ssse3IntKind::Psignw => (16, 4),
                _ => (32, 2),
            };
            let get = |value: u64, i: usize| match width {
                8 => sbyte(value, i),
                16 => sword(value, i),
                _ => sdword(value, i),
            };
            let lanes: Vec<i64> = (0..count)
                .map(|i| {
                    let control = get(src, i);
                    if control < 0 {
                        -get(dest, i)
                    } else if control == 0 {
                        0
                    } else {
                        get(dest, i)
                    }
                })
                .collect();
            pack(width, &lanes)
        }
        Ssse3IntKind::Pmaddubsw => {
            // DEST[15-0] = SaturateToSignedWord(SRC[15-8]*DEST[15-8]+SRC[7-0]*DEST[7-0]); ... SRC bytes
            // are signed, DEST bytes unsigned.
            let lanes: Vec<i64> = (0..4)
                .map(|i| {
                    saturate_to_signed_word(
                        sbyte(src, 2 * i + 1) * ubyte(dest, 2 * i + 1)
                            + sbyte(src, 2 * i) * ubyte(dest, 2 * i),
                    )
                })
                .collect();
            pack(16, &lanes)
        }
        Ssse3IntKind::Pmulhrsw => {
            // temp0[31:0] = INT32 ((DEST[15:0] * SRC[15:0]) >> 14) + 1; DEST[15:0] = temp0[16:1]
            let lanes: Vec<i64> = (0..4)
                .map(|i| {
                    let temp = ((sword(dest, i) * sword(src, i)) >> 14) + 1;
                    (temp >> 1) & 0xffff
                })
                .collect();
            pack(16, &lanes)
        }
        Ssse3IntKind::Phaddw => pack(
            16,
            &[
                sword(dest, 1) + sword(dest, 0),
                sword(dest, 3) + sword(dest, 2),
                sword(src, 1) + sword(src, 0),
                sword(src, 3) + sword(src, 2),
            ],
        ),
        Ssse3IntKind::Phsubw => pack(
            16,
            &[
                sword(dest, 0) - sword(dest, 1),
                sword(dest, 2) - sword(dest, 3),
                sword(src, 0) - sword(src, 1),
                sword(src, 2) - sword(src, 3),
            ],
        ),
        Ssse3IntKind::Phaddsw => pack(
            16,
            &[
                saturate_to_signed_word(sword(dest, 1) + sword(dest, 0)),
                saturate_to_signed_word(sword(dest, 3) + sword(dest, 2)),
                saturate_to_signed_word(sword(src, 1) + sword(src, 0)),
                saturate_to_signed_word(sword(src, 3) + sword(src, 2)),
            ],
        ),
        Ssse3IntKind::Phsubsw => pack(
            16,
            &[
                saturate_to_signed_word(sword(dest, 0) - sword(dest, 1)),
                saturate_to_signed_word(sword(dest, 2) - sword(dest, 3)),
                saturate_to_signed_word(sword(src, 0) - sword(src, 1)),
                saturate_to_signed_word(sword(src, 2) - sword(src, 3)),
            ],
        ),
        Ssse3IntKind::Phaddd => pack(
            32,
            &[
                sdword(dest, 1) + sdword(dest, 0),
                sdword(src, 1) + sdword(src, 0),
            ],
        ),
        Ssse3IntKind::Phsubd => pack(
            32,
            &[
                sdword(dest, 0) - sdword(dest, 1),
                sdword(src, 0) - sdword(src, 1),
            ],
        ),
    }
}

// ---- helpers ----

/// Runs one MMX instruction on a fresh CPU with MM1 = `dest`; the source is MM2 (register form) or the
/// quadword at `[rax]` (memory form). Returns the CPU after the step.
fn run_register(kind: &Kind, dest: u64, src: u64, immediate: u8) -> LongModeState {
    let code = reg_code(kind, 1, 2, immediate);
    let mut cpu = cpu();
    set_mm(&mut cpu, 1, dest);
    set_mm(&mut cpu, 2, src);
    let mut ram = Ram::new(&code);
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced),
        "{} register",
        kind.name
    );
    assert_eq!(cpu.rip, CODE + code.len() as u64);
    cpu
}

fn run_memory(kind: &Kind, dest: u64, src: u64, immediate: u8) -> LongModeState {
    let code = mem_code(kind, 1, immediate);
    let mut cpu = cpu();
    set_mm(&mut cpu, 1, dest);
    let mut ram = Ram::new(&code);
    ram.put(DATA, &src.to_le_bytes());
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced),
        "{} memory",
        kind.name
    );
    assert_eq!(ram.reads, vec![(DATA, 8)], "{}: one 8-byte read", kind.name);
    assert!(ram.writes.is_empty());
    cpu
}

fn assert_vector(name: &str, dest: u64, src: u64, immediate: u8, expected: u64) {
    let kind = kind_named(name);
    for (what, cpu) in [
        ("register", run_register(kind, dest, src, immediate)),
        ("memory", run_memory(kind, dest, src, immediate)),
    ] {
        assert_eq!(
            mm(&cpu, 1),
            expected,
            "{name} {what} imm {immediate}: {dest:#018x} {src:#018x}"
        );
        assert_eq!(exponent(&cpu, 1), 0xffff, "{name} {what}: exponent ones");
    }
}

fn bytes(values: [u8; 8]) -> u64 {
    u64::from_le_bytes(values)
}
fn words(values: [u16; 4]) -> u64 {
    pack(16, &values.map(i64::from))
}
fn dwords(values: [u32; 2]) -> u64 {
    pack(32, &values.map(i64::from))
}

// ---- hand-computed vectors ----

#[test]
fn pshufb_indexes_the_destination_by_the_low_three_bits_and_zeroes_on_bit_7() {
    // dest.byte[i] = 0x10 + i. Control bytes: 07 -> byte 7; 00 -> byte 0; 80 -> 0; 01 -> byte 1;
    // 08 -> 08 & 7 = byte 0 (the 128-bit form would index byte 8); 0F -> byte 7; FF -> 0; 77 -> byte 7.
    let dest = bytes([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
    let control = bytes([0x07, 0x00, 0x80, 0x01, 0x08, 0x0f, 0xff, 0x77]);
    assert_vector(
        "pshufb",
        dest,
        control,
        0,
        bytes([0x17, 0x10, 0x00, 0x11, 0x10, 0x17, 0x00, 0x17]),
    );
    // Every index 0..7 and its aliases with the upper nibble 0x70 (only bit 7 matters besides 2:0).
    for index in 0..8u8 {
        let control = bytes([index; 8]);
        assert_vector("pshufb", dest, control, 0, bytes([0x10 + index; 8]));
        assert_vector(
            "pshufb",
            dest,
            bytes([0x78 | index; 8]),
            0,
            bytes([0x10 + index; 8]),
        );
        assert_vector("pshufb", dest, bytes([0x80 | index; 8]), 0, 0);
    }
}

#[test]
fn palignr_shifts_the_dest_src_concatenation_right_by_whole_bytes() {
    // CONCATENATE(DEST,SRC) with SRC in the low half; the result is the low 64 bits after the shift.
    let src = bytes([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
    let dest = bytes([0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27]);
    assert_vector("palignr", dest, src, 0, src);
    assert_vector(
        "palignr",
        dest,
        src,
        3,
        bytes([0x13, 0x14, 0x15, 0x16, 0x17, 0x20, 0x21, 0x22]),
    );
    assert_vector("palignr", dest, src, 8, dest);
    assert_vector(
        "palignr",
        dest,
        src,
        9,
        bytes([0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x00]),
    );
    assert_vector("palignr", dest, src, 15, bytes([0x27, 0, 0, 0, 0, 0, 0, 0]));
    // 16 and above shift out the whole 128-bit concatenation: zero (a 128-bit form would still have data
    // up to 31).
    for count in [16u8, 17, 24, 31, 32, 63, 64, 128, 255] {
        assert_vector("palignr", dest, src, count, 0);
    }
}

#[test]
fn pabs_is_the_unsigned_magnitude_and_the_most_negative_value_maps_to_itself() {
    assert_vector(
        "pabsb",
        0x1111_1111_1111_1111,
        bytes([0x80, 0x81, 0x7f, 0x00, 0xff, 0x01, 0xfe, 0x80]),
        0,
        bytes([0x80, 0x7f, 0x7f, 0x00, 0x01, 0x01, 0x02, 0x80]),
    );
    assert_vector(
        "pabsw",
        !0,
        words([0x8000, 0xffff, 0x0001, 0x7fff]),
        0,
        words([0x8000, 0x0001, 0x0001, 0x7fff]),
    );
    assert_vector(
        "pabsd",
        !0,
        dwords([0x8000_0000, 0xffff_fffe]),
        0,
        dwords([0x8000_0000, 0x0000_0002]),
    );
    assert_vector(
        "pabsd",
        0,
        dwords([0x7fff_ffff, 0x8000_0001]),
        0,
        dwords([0x7fff_ffff, 0x7fff_ffff]),
    );
}

#[test]
fn psign_negates_zeroes_or_keeps_by_the_sign_of_the_source_including_zero_and_int_min() {
    assert_vector(
        "psignb",
        bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x80]),
        bytes([0x80, 0x00, 0x01, 0xff, 0x7f, 0x81, 0x00, 0xff]),
        0,
        bytes([0xff, 0x00, 0x03, 0xfc, 0x05, 0xfa, 0x00, 0x80]),
    );
    assert_vector(
        "psignw",
        words([0x1234, 0x8000, 0x0005, 0x7fff]),
        words([0x8000, 0xffff, 0x0000, 0x0005]),
        0,
        words([0xedcc, 0x8000, 0x0000, 0x7fff]),
    );
    assert_vector(
        "psignd",
        dwords([0x8000_0000, 0x0000_0007]),
        dwords([0xffff_ffff, 0x0000_0000]),
        0,
        dwords([0x8000_0000, 0x0000_0000]),
    );
    assert_vector(
        "psignd",
        dwords([0x0000_0007, 0xffff_fff9]),
        dwords([0x8000_0000, 0x7fff_ffff]),
        0,
        dwords([0xffff_fff9, 0xffff_fff9]),
    );
}

#[test]
fn pmaddubsw_multiplies_unsigned_destination_bytes_by_signed_source_bytes_and_saturates() {
    // lane 0: 255*127 + 255*127 = 64770 -> 0x7fff; lane 1: 255*(-128)*2 = -65280 -> 0x8000;
    // lane 2: 1*3 + 2*(-3) = -3; lane 3: 0*0x55 + 16*(-86) = -1376 = 0xfaa0.
    assert_vector(
        "pmaddubsw",
        bytes([0xff, 0xff, 0xff, 0xff, 0x01, 0x02, 0x00, 0x10]),
        bytes([0x7f, 0x7f, 0x80, 0x80, 0x03, 0xfd, 0x55, 0xaa]),
        0,
        words([0x7fff, 0x8000, 0xfffd, 0xfaa0]),
    );
    // Exactly representable extremes are kept: 255*(-128) + 0 = -32640; 255*127 + 0 = 32385.
    assert_vector(
        "pmaddubsw",
        bytes([0xff, 0x00, 0xff, 0x00, 0, 0, 0, 0]),
        bytes([0x80, 0x00, 0x7f, 0x00, 0, 0, 0, 0]),
        0,
        words([0x8080, 0x7e81, 0, 0]),
    );
}

#[test]
fn pmulhrsw_rounds_the_scaled_product_to_nearest_with_ties_up() {
    // (((a*b) >> 14) + 1) >> 1 per word: 0x4000^2 -> 0x2000; (-32768)^2 -> 0x8000 (bit 15 of the
    // result: 65537 >> 1 = 32768); 1*1 -> 0; 0x7fff^2 = 1073676289 -> (65532 + 1) >> 1 = 0x7ffe.
    assert_vector(
        "pmulhrsw",
        words([0x4000, 0x8000, 0x0001, 0x7fff]),
        words([0x4000, 0x8000, 0x0001, 0x7fff]),
        0,
        words([0x2000, 0x8000, 0x0000, 0x7ffe]),
    );
    // Half-way cases: 0x2000 * 1 = 8192 >> 14 = 0, +1 >> 1 = 0; 0x4000 * 1 = 16384 >> 14 = 1, +1 >> 1 = 1;
    // -1 * 0x4000 = -16384 >> 14 = -1, +1 = 0 -> 0; -0x8000 ... 0x8000 * 1 = -32768 >> 14 = -2, +1 = -1, >> 1 = -1.
    assert_vector(
        "pmulhrsw",
        words([0x2000, 0x4000, 0xffff, 0x8000]),
        words([0x0001, 0x0001, 0x4000, 0x0001]),
        0,
        words([0x0000, 0x0001, 0x0000, 0xffff]),
    );
}

#[test]
fn horizontal_adds_and_subtracts_pair_the_destination_low_and_the_source_high() {
    let d = words([1, 2, 3, 4]);
    let s = words([10, 20, 30, 40]);
    // PHADDW: the pairs of the destination in words 0..1, the pairs of the source in words 2..3.
    assert_vector("phaddw", d, s, 0, words([3, 7, 30, 70]));
    // PHADDW wraps: 0x7fff + 1 = 0x8000, 0x8000 + 0xffff = 0x7fff.
    assert_vector(
        "phaddw",
        words([0x7fff, 0x0001, 0x8000, 0xffff]),
        words([0, 0, 0xffff, 0xffff]),
        0,
        words([0x8000, 0x7fff, 0x0000, 0xfffe]),
    );
    // PHSUBW: first word minus second word of each pair: 10-3, 100-1, 5-9, 0x8000-1 (wraps).
    assert_vector(
        "phsubw",
        words([10, 3, 100, 1]),
        words([5, 9, 0x8000, 1]),
        0,
        words([7, 99, 0xfffc, 0x7fff]),
    );
    // PHADDSW saturates both ways.
    assert_vector(
        "phaddsw",
        words([0x7fff, 0x0001, 0x8000, 0xffff]),
        words([0x7000, 0x7000, 0x8000, 0x8000]),
        0,
        words([0x7fff, 0x8000, 0x7fff, 0x8000]),
    );
    // PHSUBSW: -32768-1 -> -32768; 32767-(-1) -> 32767; 0-(-32768) -> 32767; 32767-(-32768) -> 32767.
    assert_vector(
        "phsubsw",
        words([0x8000, 0x0001, 0x7fff, 0xffff]),
        words([0x0000, 0x8000, 0x7fff, 0x8000]),
        0,
        words([0x8000, 0x7fff, 0x7fff, 0x7fff]),
    );
    // Doublewords: low half from the destination, high half from the source.
    assert_vector(
        "phaddd",
        dwords([0x7fff_ffff, 0x0000_0001]),
        dwords([0xffff_ffff, 0x0000_0002]),
        0,
        dwords([0x8000_0000, 0x0000_0001]),
    );
    assert_vector(
        "phsubd",
        dwords([5, 7]),
        dwords([0, 1]),
        0,
        dwords([0xffff_fffe, 0xffff_ffff]),
    );
}

// ---- generated matrices against the independent reference ----

#[test]
fn generated_and_boundary_data_agree_with_the_reference_for_every_form() {
    let mut rng = Rng(0x5337_0001);
    let mut runs = 0;
    for kind in &KINDS {
        for mode in 0..5 {
            for round in 0..60 {
                let dest = rng.vector(mode);
                let src = rng.vector(mode);
                let immediate = match round % 4 {
                    0 => (rng.next() % 20) as u8,
                    1 => (rng.next() % 40) as u8,
                    2 => rng.next() as u8,
                    _ => [0u8, 7, 8, 9, 15, 16, 255][(rng.next() % 7) as usize],
                };
                let expected = reference64(kind.kind, dest, src, immediate);
                for (what, cpu) in [
                    ("register", run_register(kind, dest, src, immediate)),
                    ("memory", run_memory(kind, dest, src, immediate)),
                ] {
                    assert_eq!(
                        mm(&cpu, 1),
                        expected,
                        "{} {what} mode {mode} imm {immediate}: {dest:#018x} {src:#018x}",
                        kind.name
                    );
                    runs += 1;
                }
            }
        }
    }
    assert_eq!(runs, 16 * 5 * 60 * 2);
}

#[test]
fn every_register_pair_of_the_register_forms_changes_only_the_destination() {
    for kind in &KINDS {
        for destination in 0u8..8 {
            for source in 0u8..8 {
                let code = reg_code(kind, destination, source, 5);
                let mut cpu = cpu();
                let before = cpu.clone();
                let dest_value = mm(&cpu, destination.into());
                let src_value = mm(&cpu, source.into());
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Advanced)
                );
                let name = format!("{} mm{destination}, mm{source}", kind.name);
                assert_eq!(
                    mm(&cpu, destination.into()),
                    reference64(kind.kind, dest_value, src_value, 5),
                    "{name}"
                );
                // Everything except MMd, the TOP/tag effect of an MMX instruction and RIP is unchanged:
                // the source register keeps its payload and its exponent (also when it is the destination
                // register of an earlier write, the exponent of a source is not rewritten).
                for index in 0..8 {
                    if index == usize::from(destination) {
                        assert_eq!(exponent(&cpu, index), 0xffff, "{name}");
                    } else {
                        assert_eq!(
                            cpu.x87.as_ref().unwrap().registers[index],
                            before.x87.as_ref().unwrap().registers[index],
                            "{name}: register {index}"
                        );
                    }
                }
                let mut expected = before.clone();
                {
                    let x87 = expected.x87.as_mut().unwrap();
                    x87.enter_mmx_state();
                    x87.write_mmx_register(
                        destination,
                        reference64(kind.kind, dest_value, src_value, 5).to_le_bytes(),
                    );
                }
                expected.rip = CODE + code.len() as u64;
                assert_eq!(
                    cpu, expected,
                    "{name}: only the destination and the x87 control state"
                );
                assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
            }
        }
    }
}

// ---- addressing, prefixes ----

enum Disp {
    None,
    D8(i8),
    D32(i32),
    /// RIP-relative: the absolute target; disp32 is computed from the end of the instruction.
    Rip(u64),
}

struct Form {
    name: &'static str,
    /// Legacy prefixes before the opcode (67, null segment overrides).
    extra: &'static [u8],
    /// REX bits W/X/B; 0 means no REX byte.
    rex: u8,
    mode: u8,
    rm: u8,
    sib: Option<u8>,
    disp: Disp,
    regs: &'static [(Register, u64)],
    address: u64,
}

const FORMS: &[Form] = &[
    Form {
        name: "[rax]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8010)],
        address: 0x8010,
    },
    Form {
        name: "[rbx+0x120] disp32",
        extra: &[],
        rex: 0,
        mode: 2,
        rm: 3,
        sib: None,
        disp: Disp::D32(0x120),
        regs: &[(Register::Rbx, 0x8000)],
        address: 0x8120,
    },
    Form {
        name: "[rbp+0x10]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::Rbp, 0x8320)],
        address: 0x8330,
    },
    Form {
        name: "[rsp] (SIB)",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x24),
        disp: Disp::None,
        regs: &[(Register::Rsp, 0x8230)],
        address: 0x8230,
    },
    Form {
        name: "[rax+rcx*4+0x10]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 4,
        sib: Some(0x88),
        disp: Disp::D8(0x10),
        regs: &[(Register::Rax, 0x8000), (Register::Rcx, 4)],
        address: 0x8020,
    },
    Form {
        name: "[r8] (REX.B)",
        extra: &[],
        rex: 0x01,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::R8, 0x8440)],
        address: 0x8440,
    },
    Form {
        name: "[r13+0x10] (REX.B, disp8)",
        extra: &[],
        rex: 0x01,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::R13, 0x8650)],
        address: 0x8660,
    },
    Form {
        name: "[r12] (REX.B, SIB)",
        extra: &[],
        rex: 0x01,
        mode: 0,
        rm: 4,
        sib: Some(0x24),
        disp: Disp::None,
        regs: &[(Register::R12, 0x8570)],
        address: 0x8570,
    },
    Form {
        name: "[rbx+r9*8+0x20] (REX.X)",
        extra: &[],
        rex: 0x02,
        mode: 1,
        rm: 4,
        sib: Some(0xcb),
        disp: Disp::D8(0x20),
        regs: &[(Register::Rbx, 0x8100), (Register::R9, 2)],
        address: 0x8130,
    },
    Form {
        name: "[disp32] (SIB, no base)",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x25),
        disp: Disp::D32(0x8800),
        regs: &[],
        address: 0x8800,
    },
    Form {
        name: "[rip+disp32]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x8900),
        regs: &[],
        address: 0x8900,
    },
    Form {
        name: "[rip-disp32] backward",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x1800),
        regs: &[],
        address: 0x1800,
    },
    Form {
        name: "[rax] (REX.W ignored)",
        extra: &[],
        rex: 0x08,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8a40)],
        address: 0x8a40,
    },
    Form {
        name: "[eax] (67, upper bits ignored)",
        extra: &[0x67],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0xdead_beef_0000_8b00)],
        address: 0x8b00,
    },
    Form {
        name: "[rax] CS",
        extra: &[0x2e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c00)],
        address: 0x8c00,
    },
    Form {
        name: "[rax] SS",
        extra: &[0x36],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c10)],
        address: 0x8c10,
    },
    Form {
        name: "[rax] DS",
        extra: &[0x3e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c20)],
        address: 0x8c20,
    },
    Form {
        name: "[rax] ES",
        extra: &[0x26],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c30)],
        address: 0x8c30,
    },
];

fn form_code(kind: &Kind, form: &Form, destination: u8, immediate: u8) -> Vec<u8> {
    let mut code = form.extra.to_vec();
    if form.rex != 0 {
        code.push(0x40 | form.rex);
    }
    code.extend_from_slice(&kind.opcode);
    code.push((form.mode << 6) | (destination << 3) | form.rm);
    if let Some(sib) = form.sib {
        code.push(sib);
    }
    match form.disp {
        Disp::None => {}
        Disp::D8(value) => code.push(value as u8),
        Disp::D32(value) => code.extend_from_slice(&value.to_le_bytes()),
        Disp::Rip(target) => {
            // RIP-relative is relative to the end of the whole instruction, immediate included.
            let end = CODE + code.len() as u64 + 4 + u64::from(kind.immediate());
            code.extend_from_slice(&(target.wrapping_sub(end) as i32).to_le_bytes());
        }
    }
    if kind.immediate() {
        code.push(immediate);
    }
    code
}

fn form_cpu(form: &Form) -> LongModeState {
    let mut cpu = cpu();
    for (register, value) in form.regs {
        cpu.gpr[*register as usize] = *value;
    }
    cpu
}

#[test]
fn every_addressing_form_loads_the_quadword_source_for_all_sixteen_instructions() {
    for kind in &KINDS {
        for form in FORMS {
            let name = format!("{} {}", kind.name, form.name);
            let code = form_code(kind, form, 3, 2);
            let mut cpu = form_cpu(form);
            let dest = mm(&cpu, 3);
            let src = 0x0123_4567_89ab_cdefu64.rotate_left(form.address as u32 & 63);
            let mut ram = Ram::new(&code);
            ram.put(form.address, &src.to_le_bytes());
            let before = ram.bytes.clone();
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{name}"
            );
            assert_eq!(cpu.rip, CODE + code.len() as u64, "{name}");
            assert_eq!(mm(&cpu, 3), reference64(kind.kind, dest, src, 2), "{name}");
            assert_eq!(ram.reads, vec![(form.address, 8)], "{name}");
            assert!(ram.writes.is_empty(), "{name}");
            assert_eq!(ram.bytes, before, "{name}");
        }
    }
}

#[test]
fn a_quadword_source_needs_no_alignment_at_any_offset() {
    // Table 2-21 note: the #GP for a misaligned operand is the 128-bit rule; an MMX operand of 8 bytes at
    // CPL0 runs at every byte offset (and across a 16-byte and a 4 KiB boundary).
    for kind in &KINDS {
        for address in (DATA..DATA + 24).chain([0x8ff9, 0x8ffc, 0x8fff]) {
            let code = mem_code(kind, 4, 6);
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = address;
            let dest = mm(&cpu, 4);
            let mut ram = Ram::new(&code);
            let src = 0xf0e1_d2c3_b4a5_9687u64;
            ram.put(address, &src.to_le_bytes());
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{} at {address:#x}",
                kind.name
            );
            assert_eq!(mm(&cpu, 4), reference64(kind.kind, dest, src, 6));
        }
    }
}

#[test]
fn at_cpl3_with_the_alignment_check_armed_a_misaligned_quadword_is_alignment_check_and_an_aligned_one_runs()
 {
    // ADR-0023 C1 (decision 4): the misaligned source is #AC(0) without a change (the physical oracle
    // gives SIGBUS/ADRALN), the aligned one and CPL0 are unaffected by RFLAGS.AC.
    let kind = kind_named("pshufb");
    for (privilege, address, runs) in [
        (PrivilegeLevel::Ring3, DATA, true),
        (PrivilegeLevel::Ring3, DATA + 4, false),
        (PrivilegeLevel::Ring3, DATA + 7, false),
        (PrivilegeLevel::Ring0, DATA + 4, true),
    ] {
        let code = mem_code(kind, 1, 0);
        let mut cpu = cpu();
        cpu.rflags |= 1 << 18;
        cpu.gpr[Register::Rax as usize] = address;
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        let outcome = step(&mut cpu, &mut ram, privilege);
        if runs {
            assert_eq!(
                outcome,
                Ok(StepOutcome::Advanced),
                "{privilege:?} {address:#x}"
            );
        } else {
            assert_eq!(
                outcome,
                Ok(StepOutcome::Fault(od_isa::ArchitecturalFault {
                    vector: 17,
                    error_code: Some(0),
                    linear_address: None
                })),
                "{privilege:?} {address:#x}"
            );
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty());
        }
    }
}

#[test]
fn register_forms_ignore_rex_bits_and_null_segment_and_address_overrides() {
    // MM0..MM7 have no REX extension: REX.R/B/X/W do not select MM8..MM15 (they do not exist); 67 and
    // CS/DS/ES/SS overrides have no operand to act on.
    for kind in &KINDS {
        let plain = reg_code(kind, 1, 2, 3);
        let mut reference_cpu = cpu();
        let mut ram = Ram::new(&plain);
        step(&mut reference_cpu, &mut ram, PrivilegeLevel::Ring0).unwrap();
        for prefix in [
            &[0x40u8][..],
            &[0x41],
            &[0x44],
            &[0x42],
            &[0x48],
            &[0x4d],
            &[0x4f],
            &[0x67],
            &[0x2e],
            &[0x3e],
            &[0x26],
            &[0x36],
            &[0x67, 0x48],
        ] {
            let code = [prefix, &plain[..]].concat();
            let mut cpu = cpu();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{} {prefix:02x?}",
                kind.name
            );
            let mut expected = reference_cpu.clone();
            expected.rip = CODE + code.len() as u64;
            assert_eq!(cpu, expected, "{} {prefix:02x?}", kind.name);
        }
    }
}

// ---- faults ----

#[test]
fn non_canonical_addresses_fault_before_memory_and_leave_the_state() {
    for kind in &KINDS {
        for (base, vector) in [
            (Register::Rax, 13u8),
            (Register::Rsp, 12),
            (Register::Rbp, 12),
        ] {
            // [rax] is mod=0 rm=0; [rsp] needs a SIB; [rbp+0] needs a disp8.
            let mut code = kind.opcode.to_vec();
            match base {
                Register::Rax => code.push(0x08),
                Register::Rsp => code.extend_from_slice(&[0x0c, 0x24]),
                _ => code.extend_from_slice(&[0x4d, 0x00]),
            }
            if kind.immediate() {
                code.push(1);
            }
            for address in [
                0x0000_8000_0000_0000u64,
                0xffff_7fff_ffff_fff0,
                0x1234_5678_1234_5670,
            ] {
                let mut cpu = cpu();
                cpu.gpr[base as usize] = address;
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(fault(vector, Some(0))),
                    "{} {base:?} {address:#x}",
                    kind.name
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn backend_faults_leave_the_cpu_and_the_x87_state_untouched() {
    for kind in &KINDS {
        let code = mem_code(kind, 1, 0);
        let mut cpu = cpu();
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(DATA & !0xfff);
        let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert!(
            matches!(&outcome, Ok(StepOutcome::Fault(f)) if f.vector == 14),
            "{}: {outcome:?}",
            kind.name
        );
        assert_eq!(
            cpu, before,
            "{}: no TOP, tag or register change on a fault",
            kind.name
        );
    }
}

#[test]
fn lock_is_ud_for_every_form_and_prefix_order() {
    for kind in &KINDS {
        let tail: &[u8] = if kind.immediate() { &[0x04] } else { &[] };
        for (code, what) in [
            (
                [&[0xf0][..], &kind.opcode, &[0x08], tail].concat(),
                "memory",
            ),
            (
                [&[0xf0][..], &kind.opcode, &[0xc1], tail].concat(),
                "register",
            ),
            (
                [&[0xf0, 0x48][..], &kind.opcode, &[0x08], tail].concat(),
                "REX",
            ),
            (
                [&[0xf0][..], &kind.opcode, &[0x05, 0, 0, 0, 0], tail].concat(),
                "rip",
            ),
        ] {
            let mut cpu = cpu();
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(ud()),
                "{} LOCK {what}",
                kind.name
            );
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty());
        }
    }
}

#[test]
fn encodings_that_are_not_claimed_stop_as_unsupported_without_change() {
    let mut refused: Vec<(Vec<u8>, String)> = vec![];
    for kind in &KINDS {
        let tail: &[u8] = if kind.immediate() { &[0x04] } else { &[] };
        let name = kind.name;
        // F2 / F3 on the opcodes, repeated prefixes, FS and GS (no segment base in this operand model).
        for (prefix, what) in [
            (&[0xf2u8][..], "F2"),
            (&[0xf3], "F3"),
            (&[0xf2, 0xf3], "F2 F3"),
            (&[0x64], "FS"),
            (&[0x65], "GS"),
            (&[0x67, 0x67], "duplicate 67"),
            (&[0x2e, 0x3e], "two segment overrides"),
            (&[0x40, 0x40], "duplicate REX"),
            (&[0xf0, 0xf0], "duplicate LOCK"),
        ] {
            for modrm in [0x00u8, 0xc1] {
                refused.push((
                    [prefix, &kind.opcode, &[modrm], tail].concat(),
                    format!("{name} {what} modrm {modrm:#x}"),
                ));
            }
        }
    }
    for (code, name) in &refused {
        let mut cpu = cpu();
        let before = cpu.clone();
        let mut ram = Ram::new(code);
        let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
        // `duplicate LOCK` is decoder-refused or #UD, never an execution: only the change matters there.
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{name}: {outcome:?}"
        );
        assert_eq!(cpu, before, "{name}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
    }
}

#[test]
fn without_x87_state_the_mmx_forms_stop_as_unsupported_and_sse_state_is_not_needed() {
    for kind in &KINDS {
        let code = reg_code(kind, 1, 2, 3);
        let mut without_x87 = cpu();
        without_x87.x87 = None;
        let before = without_x87.clone();
        let mut ram = Ram::new(&code);
        assert!(matches!(
            step(&mut without_x87, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Unsupported { .. })
        ));
        assert_eq!(without_x87, before, "{}", kind.name);
        // The MMX register file is the x87 data register file: the SSE state plays no part.
        let mut without_sse = cpu();
        without_sse.sse = None;
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut without_sse, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{}",
            kind.name
        );
    }
}

// ---- the decoder ----

#[test]
fn the_decoder_distinguishes_the_mmx_form_from_the_xmm_form_by_the_mandatory_prefix() {
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    // PALIGNR mm1, mm2, 0x1f: REX.R and REX.B (here set) do not extend MM registers.
    let instruction = decode(&[0x45, 0x0f, 0x3a, 0x0f, 0xca, 0x1f]);
    assert_eq!(instruction.length, 6);
    assert_eq!(
        instruction.operation,
        Operation::MmxSsse3 {
            kind: Ssse3IntKind::Palignr,
            destination: 1,
            source: Sse3Source::Register(2),
            immediate: 0x1f,
        }
    );
    // The same opcode with 66 is the XMM form and keeps its REX extension.
    let instruction = decode(&[0x66, 0x45, 0x0f, 0x3a, 0x0f, 0xca, 0x1f]);
    assert_eq!(instruction.length, 7);
    assert_eq!(
        instruction.operation,
        Operation::Ssse3Int {
            kind: Ssse3IntKind::Palignr,
            destination: 9,
            source: Sse3Source::Register(10),
            immediate: 0x1f,
        }
    );
    // PSHUFB mm0, [rax+rcx*4+0x10]: no immediate, memory source.
    let instruction = decode(&[0x0f, 0x38, 0x00, 0x44, 0x88, 0x10]);
    assert_eq!(instruction.length, 6);
    assert!(matches!(
        instruction.operation,
        Operation::MmxSsse3 {
            kind: Ssse3IntKind::Pshufb,
            destination: 0,
            source: Sse3Source::Memory(_),
            immediate: 0,
        }
    ));
    // PABSD mm7, [rip+0]: three opcode bytes, ModR/M and disp32 make 8 bytes.
    let instruction = decode(&[0x0f, 0x38, 0x1e, 0x3d, 0, 0, 0, 0]);
    assert_eq!(instruction.length, 8);
    assert!(matches!(
        instruction.operation,
        Operation::MmxSsse3 {
            kind: Ssse3IntKind::Pabsd,
            destination: 7,
            ..
        }
    ));
    assert_eq!(instruction.opcode.byte, 0x1e);
}
