//! S2-32b: FBLD and FBSTP (bare `DF 20` / `DF 30` with the operand at [RAX]) in the bounded local x87 profile.
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200): Vol. 2A "FBLD" (p. 3-314)
//! and "FBSTP" (p. 3-316) (packed BCD: 18 digits, sign in bit 7 of byte 9, bits 6:0 of byte 9 ignored on the oracle; the
//! rounding control rounds the stored integer; FBSTP pops), Vol. 1 section 4.7 (packed BCD integers) and 8.3.4.
//! The physical oracle is the Core i5-3210M (`data/x87_bcd_oracle.c`, operands from
//! `data/x87_bcd_oracle_gen.py`); `data/x87_bcd_oracle_i5_3210m.txt` holds 335 answers, one per line:
//! `op fcw sw0 operand result fsw ftw`. Every row is reproduced bit for bit: the 10 result bytes, the
//! status word (TOP, C0-C3, PE) and the tag word. Operands that FBLD cannot decode (a digit above 9) and FBSTP
//! values that need 19 digits stay refused.

use od_isa::execute::{StepOutcome, step};
use od_isa::long_decode::{self, DecodeError, Operation};
use od_isa::state::{X87State, X87Tag};
use od_isa::{AccessContext, AccessKind, GuestMemory, LongModeState, MemoryError, Register};

const ORACLE: &str = include_str!("data/x87_bcd_oracle_i5_3210m.txt");
const RIP: u64 = 0x1000;
const DATA: u64 = 0x8000;

struct Ram {
    bytes: Vec<u8>,
    writes: Vec<(u64, usize)>,
}

impl Ram {
    fn new(code: &[u8], data: &[u8]) -> Self {
        let mut bytes = vec![0xcc; 0x10000];
        bytes[RIP as usize..RIP as usize + code.len()].copy_from_slice(code);
        bytes[DATA as usize..DATA as usize + data.len()].copy_from_slice(data);
        Self {
            bytes,
            writes: vec![],
        }
    }
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
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }
    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        self.writes.push((address, data.len()));
        let start = address as usize;
        self.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn hex(text: &str) -> [u8; 10] {
    let mut out = [0u8; 10];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap();
    }
    out
}

fn is_zero(raw: [u8; 10]) -> bool {
    raw[..8] == [0; 8] && raw[8] == 0 && raw[9] & 0x7f == 0
}

/// FNINIT + FLDENV(fcw, sw0, all tags empty), then (FBSTP) the FLDT push of the operand, which clears C1.
fn loaded(fcw: u16, sw0: u16, push: Option<[u8; 10]>) -> LongModeState {
    let mut x87 = X87State::fixture_seed();
    x87.control_word = fcw;
    x87.tag_word = 0xffff;
    x87.status_word = sw0;
    if let Some(raw) = push {
        let top = (((sw0 >> 11) & 7) as u8).wrapping_sub(1) & 7;
        x87.registers[usize::from(top)] = raw;
        x87.set_tag(
            top,
            if is_zero(raw) {
                X87Tag::Zero
            } else {
                X87Tag::Valid
            },
        );
        x87.status_word = (sw0 & !0x3800 & !0x0200) | (u16::from(top) << 11);
    }
    let mut state = LongModeState {
        rip: RIP,
        x87: Some(x87),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = DATA;
    state
}

fn execute(state: &mut LongModeState, ram: &mut Ram) -> Result<StepOutcome, MemoryError> {
    step(state, ram, AccessContext::default())
}

#[test]
fn oracle_rows_are_reproduced_bit_for_bit() {
    let (mut fbld, mut fbstp, mut inexact, mut rounded_up) = (0, 0, 0, 0);
    for line in ORACLE.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let fcw = u16::from_str_radix(f[1], 16).unwrap();
        let sw0 = u16::from_str_radix(f[2], 16).unwrap();
        let operand = hex(f[3]);
        let hw_result = hex(f[4]);
        let hw_sw = u16::from_str_radix(f[5], 16).unwrap();
        let hw_tw = u16::from_str_radix(f[6], 16).unwrap();
        match f[0] {
            "fbld" => {
                fbld += 1;
                let mut state = loaded(fcw, sw0, None);
                let mut ram = Ram::new(&[0xdf, 0x20], &operand);
                assert_eq!(
                    execute(&mut state, &mut ram),
                    Ok(StepOutcome::Advanced),
                    "{line}"
                );
                let x87 = state.x87.as_ref().unwrap();
                assert_eq!(x87.registers[usize::from(x87.top())], hw_result, "{line}");
                assert_eq!(x87.status_word, hw_sw, "{line}");
                assert_eq!(x87.tag_word, hw_tw, "{line}");
                assert_eq!(
                    (state.rip, x87.instruction_pointer, x87.data_pointer),
                    (RIP + 2, RIP, DATA)
                );
                assert!(ram.writes.is_empty());
            }
            "fbstp" => {
                fbstp += 1;
                let mut state = loaded(fcw, sw0, Some(operand));
                let before = state.x87.clone().unwrap();
                let mut ram = Ram::new(&[0xdf, 0x30], &[0xee; 10]);
                assert_eq!(
                    execute(&mut state, &mut ram),
                    Ok(StepOutcome::Advanced),
                    "{line}"
                );
                let x87 = state.x87.as_ref().unwrap();
                assert_eq!(
                    ram.bytes[DATA as usize..DATA as usize + 10],
                    hw_result,
                    "{line}"
                );
                assert_eq!(ram.writes, [(DATA, 10)], "one 10-byte write: {line}");
                assert_eq!(x87.status_word, hw_sw, "{line}");
                assert_eq!(x87.tag_word, hw_tw, "{line}");
                assert_eq!(
                    x87.registers, before.registers,
                    "the register payloads survive the pop"
                );
                inexact += u32::from(hw_sw & 0x20 != 0);
                rounded_up += u32::from(hw_sw & 0x200 != 0);
            }
            other => panic!("{other}"),
        }
    }
    // The rows cover both the exact and the rounded stores and every rounding direction.
    assert_eq!((fbld, fbstp, inexact, rounded_up), (117, 218, 208, 65));
}

#[test]
fn refusals_leave_no_trace() {
    let valid = [0x45, 0x23, 0x01, 0, 0, 0, 0, 0, 0, 0];
    let one = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    let big = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x3b, 0x40]; // 2^60
    let cases: Vec<(&str, [u8; 10], Box<dyn Fn(&mut LongModeState)>)> = vec![
        (
            "fbld",
            [0x4a, 0x23, 0x01, 0, 0, 0, 0, 0, 0, 0],
            Box::new(|_| {}),
        ), // digit A in the low nibble
        (
            "fbld",
            [0x45, 0xa3, 0x01, 0, 0, 0, 0, 0, 0, 0],
            Box::new(|_| {}),
        ), // digit A in the high nibble
        ("fbld", [0, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0], Box::new(|_| {})),
        ("fbld", valid, Box::new(|s| s.x87 = None)),
        (
            "fbld",
            valid,
            Box::new(|s| {
                s.x87.as_mut().unwrap().set_tag(7, X87Tag::Valid);
            }),
        ), // stack overflow
        (
            "fbld",
            valid,
            Box::new(|s| s.x87.as_mut().unwrap().control_word = 0x037e),
        ),
        (
            "fbld",
            valid,
            Box::new(|s| s.x87.as_mut().unwrap().status_word |= 0x0004),
        ),
        (
            "fbld",
            valid,
            Box::new(|s| s.x87.as_mut().unwrap().instruction_selector = Some(0)),
        ),
        ("fbstp", one, Box::new(|s| s.x87 = None)),
        (
            "fbstp",
            one,
            Box::new(|s| {
                s.x87.as_mut().unwrap().set_tag(7, X87Tag::Empty);
            }),
        ),
        (
            "fbstp",
            one,
            Box::new(|s| s.x87.as_mut().unwrap().control_word = 0x0360),
        ),
        (
            "fbstp",
            one,
            Box::new(|s| s.x87.as_mut().unwrap().status_word |= 0x0001),
        ),
        (
            "fbstp",
            one,
            Box::new(|s| s.x87.as_mut().unwrap().data_selector = Some(0)),
        ),
        // 10^18 and larger (IE with the BCD indefinite), the denormal, infinity and NaN encodings.
        (
            "fbstp",
            [0, 0, 0x40, 0x76, 0x3a, 0x6b, 0x0b, 0xde, 0x3a, 0x40], // exactly 10^18
            Box::new(|_| {}),
        ),
        ("fbstp", big, Box::new(|_| {})),
        ("fbstp", [1, 0, 0, 0, 0, 0, 0, 0, 0, 0], Box::new(|_| {})),
        (
            "fbstp",
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            Box::new(|_| {}),
        ),
        (
            "fbstp",
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            Box::new(|_| {}),
        ),
        (
            "fbstp",
            [0, 0, 0, 0, 0, 0, 0, 0x40, 0xff, 0x3f],
            Box::new(|_| {}),
        ),
    ];
    for (index, (op, operand, tweak)) in cases.into_iter().enumerate() {
        let mut state = if op == "fbld" {
            loaded(0x037f, 0, None)
        } else {
            loaded(0x037f, 0, Some(operand))
        };
        tweak(&mut state);
        let mut ram = Ram::new(
            &[0xdf, if op == "fbld" { 0x20 } else { 0x30 }],
            if op == "fbld" { &operand } else { &[0xee; 10] },
        );
        let before = (state.clone(), ram.bytes.clone());
        let outcome = execute(&mut state, &mut ram).unwrap();
        assert_eq!(
            outcome,
            StepOutcome::Unsupported { opcode: 0xdf },
            "case {index} {op}"
        );
        assert_eq!((state, ram.bytes), before, "case {index} {op}");
        assert!(ram.writes.is_empty(), "case {index}");
    }
}

#[test]
fn memory_faults_and_decoder() {
    // A non-canonical [RAX] stops FBLD and FBSTP with #GP before any change.
    for (code, operand) in [
        ([0xdf, 0x20], None),
        ([0xdf, 0x30], Some([0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f])),
    ] {
        let mut state = loaded(0x037f, 0, operand);
        state.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut ram = Ram::new(&code, &[]);
        let outcome = execute(&mut state, &mut ram).unwrap();
        assert!(
            matches!(outcome, StepOutcome::Fault(fault) if fault.vector == 13),
            "{code:02x?}"
        );
        assert_eq!(state, before);
    }
    let decode =
        |code: &[u8]| long_decode::decode(&mut Ram::new(code, &[]), RIP, AccessContext::default());
    assert_eq!(
        decode(&[0xdf, 0x20]).unwrap().operation,
        Operation::FbldM80Rax
    );
    assert_eq!(
        decode(&[0xdf, 0x30]).unwrap().operation,
        Operation::FbstpM80Rax
    );
    assert_eq!(decode(&[0xdf, 0x20]).unwrap().length, 2);
    for prefix in [0x66u8, 0xf2, 0xf3, 0x2e, 0x67, 0x48, 0x64] {
        assert!(
            matches!(
                decode(&[prefix, 0xdf, 0x20]),
                Err(DecodeError::Unsupported { .. })
            ),
            "{prefix:02x}"
        );
        assert!(
            matches!(
                decode(&[prefix, 0xdf, 0x30]),
                Err(DecodeError::Unsupported { .. })
            ),
            "{prefix:02x}"
        );
    }
    // The other memory operands of the group stay undecoded.
    for modrm in [
        0x21,
        0x22,
        0x24,
        0x31,
        0x32,
        0x20 | 0x40,
        0x30 | 0x80,
        0xe0 | 0x10,
    ] {
        let result = decode(&[0xdf, modrm, 0, 0, 0, 0]);
        assert!(
            !matches!(
                result.as_ref().map(|i| i.operation),
                Ok(Operation::FbldM80Rax | Operation::FbstpM80Rax)
            ),
            "{modrm:02x}"
        );
    }
}
