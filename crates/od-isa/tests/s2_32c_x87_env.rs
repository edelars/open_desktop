//! S2-32c: FNSTENV, FNSAVE and FRSTOR (bare `D9 30`, `DD 30`, `DD 20` with the image at [RAX], no prefix) in the
//! bounded local x87 profile.
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200): Vol. 1 section 8.1.8 (the x87
//! pointers: FSTENV/FNSTENV and FSAVE/FNSAVE save the low 32 bits of FIP and FDP, FCS and FDS when CR0.PE = 1, saved as
//! 0000H when CPUID.07H.00H:EBX[13] deprecates them; FSAVE/FNSAVE clear FIP, FCS, FDP, FDS afterwards; FLDENV/FRSTOR load
//! the low 32 bits and clear the upper 32), section 8.1.10 and Figure 8-9 (the 32-bit protected-mode image), Vol. 2A
//! "FSTENV/FNSTENV" (p. 3-390: the instruction masks all exceptions after saving; FSTENV is FWAIT plus FNSTENV),
//! "FSAVE/FNSAVE" (p. 3-375: the registers follow in ST(0)..ST(7) order, then FNINIT values) and "FRSTOR" (p. 3-373).
//! CR0.EM/TS (#NM) and CPL3 #AC are not modeled, like in the neighbouring bounded x87 forms.
//!
//! Policy for FCS and FDS (S2-32c): the image carries the represented selector (`Some(v)`) and 0000H when none is
//! represented (`None`), which is also what a processor with FCS/FDS deprecation saves; FRSTOR loads a selector as
//! `Some(v)`, except that a zero selector stays unrepresented when it was. The physical oracle is the Core i5-3210M
//! (`data/x87_env_oracle.c`, images from `data/x87_env_oracle_gen.py`): each row of `data/x87_env_oracle_i5_3210m.txt` is
//! `FNINIT; FRSTOR in; FNSTENV env; FNSAVE save; FNSTENV env2` as `in env save env2` in hex. That CPU does not deprecate
//! FCS/FDS, so the rows use images (not instruction sequences, which would save its real CS 0033H and DS) and every byte of
//! the three outputs is compared, reserved bytes (FFFFH on this CPU) and the recomputed tag word included.

use od_isa::execute::{StepOutcome, step};
use od_isa::long_decode::{self, DecodeError, Operation};
use od_isa::state::{X87State, X87Tag};
use od_isa::{AccessContext, AccessKind, GuestMemory, LongModeState, MemoryError, Register};

const ORACLE: &str = include_str!("data/x87_env_oracle_i5_3210m.txt");
const RIP: u64 = 0x1000;
const IN: u64 = 0x4000;
const ENV: u64 = 0x5000;
const SAVE: u64 = 0x6000;
const ENV2: u64 = 0x7000;

struct Ram {
    bytes: Vec<u8>,
    writes: Vec<(u64, usize)>,
    reads: Vec<(u64, usize)>,
}

impl Ram {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![0xcc; 0x10000];
        bytes[RIP as usize..RIP as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            writes: vec![],
            reads: vec![],
        }
    }
    fn put(&mut self, address: u64, data: &[u8]) {
        self.bytes[address as usize..address as usize + data.len()].copy_from_slice(data);
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
        let Some(source) = self.bytes.get(start..start + out.len()) else {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            });
        };
        out.copy_from_slice(source);
        self.reads.push((address, out.len()));
        Ok(())
    }
    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        let start = address as usize;
        let Some(target) = self.bytes.get_mut(start..start + data.len()) else {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            });
        };
        target.copy_from_slice(data);
        self.writes.push((address, data.len()));
        Ok(())
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap())
        .collect()
}

fn state_with(x87: Option<X87State>, rax: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: RIP,
        x87,
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = rax;
    state
}

fn run(state: &mut LongModeState, ram: &mut Ram) -> Result<StepOutcome, MemoryError> {
    step(state, ram, AccessContext::default())
}

const FRSTOR: [u8; 2] = [0xdd, 0x20];
const FNSAVE: [u8; 2] = [0xdd, 0x30];
const FNSTENV: [u8; 2] = [0xd9, 0x30];

/// Execute one two-byte instruction at RIP with RAX = address; the state's RIP is reset first.
fn exec(state: &mut LongModeState, ram: &mut Ram, code: [u8; 2], address: u64) -> StepOutcome {
    state.rip = RIP;
    state.gpr[Register::Rax as usize] = address;
    ram.put(RIP, &code);
    run(state, ram).unwrap()
}

#[test]
fn oracle_rows_are_reproduced_byte_for_byte() {
    let (mut rows, mut zero_fcs, mut zero_fds, mut empty_tags, mut flagged) = (0, 0, 0, 0, 0);
    for line in ORACLE.lines() {
        let fields: Vec<Vec<u8>> = line.split_whitespace().map(hex).collect();
        let [image, env, save, env2] = &fields[..] else {
            panic!("{line}")
        };
        assert_eq!(
            (image.len(), env.len(), save.len(), env2.len()),
            (108, 28, 108, 28)
        );
        rows += 1;
        zero_fcs += usize::from(image[16..18] == [0, 0]);
        zero_fds += usize::from(image[24..26] == [0, 0]);
        empty_tags += usize::from(env[8..10] == [0xff, 0xff]);
        flagged += usize::from(image[4] & 0x7f != 0);
        // The starting selector state must not matter: FRSTOR overwrites the whole environment.
        for selectors in [(None, None), (Some(0x5a5a), Some(0xa5a5))] {
            let mut x87 = X87State::fixture_seed();
            x87.instruction_selector = selectors.0;
            x87.data_selector = selectors.1;
            let mut state = state_with(Some(x87), IN);
            let mut ram = Ram::new(&[]);
            ram.put(IN, image);
            assert_eq!(
                exec(&mut state, &mut ram, FRSTOR, IN),
                StepOutcome::Advanced,
                "{line}"
            );
            assert_eq!(ram.reads, [(IN, 108)], "one 108-byte read: {line}");
            assert_eq!(state.rip, RIP + 2);
            assert_eq!(
                exec(&mut state, &mut ram, FNSTENV, ENV),
                StepOutcome::Advanced,
                "{line}"
            );
            assert_eq!(
                &ram.bytes[ENV as usize..ENV as usize + 28],
                &env[..],
                "env {line}"
            );
            let after_env = state.x87.clone().unwrap();
            assert_eq!(
                exec(&mut state, &mut ram, FNSAVE, SAVE),
                StepOutcome::Advanced,
                "{line}"
            );
            assert_eq!(
                &ram.bytes[SAVE as usize..SAVE as usize + 108],
                &save[..],
                "save {line}"
            );
            // FNSAVE re-initialises: FNINIT values, FIP, FDP and the known selectors cleared.
            let x87 = state.x87.as_ref().unwrap();
            assert_eq!(
                (x87.control_word, x87.status_word, x87.tag_word),
                (0x037f, 0, 0xffff)
            );
            assert_eq!(
                (x87.instruction_pointer, x87.data_pointer, x87.last_opcode),
                (0, 0, 0)
            );
            assert!(x87.instruction_selector.is_none_or(|v| v == 0));
            assert!(x87.data_selector.is_none_or(|v| v == 0));
            assert_eq!(
                exec(&mut state, &mut ram, FNSTENV, ENV2),
                StepOutcome::Advanced,
                "{line}"
            );
            assert_eq!(
                &ram.bytes[ENV2 as usize..ENV2 as usize + 28],
                &env2[..],
                "env2 {line}"
            );
            assert_eq!(
                ram.writes,
                [(ENV, 28), (SAVE, 108), (ENV2, 28)],
                "one write per image: {line}"
            );
            // FNSTENV changed nothing it did not have to: the all-masked control word is unchanged.
            assert_eq!(after_env.control_word | 0x3f, after_env.control_word);
        }
    }
    // The rows cover zero and non-zero selectors, empty-tag images and sticky exception flags.
    assert_eq!(
        (rows, zero_fcs, zero_fds, empty_tags, flagged),
        (324, 68, 84, 48, 319)
    );
}

#[test]
fn fnstenv_image_of_the_fixture_state_and_the_selector_policy() {
    // fixture_seed: CW 027F, SW 3805, FIP 0123456789ABCDEF, FDP FEDCBA9876543210, FOP 05AA, tags 1BE4
    // with register contents that change the non-empty classes (R1 and R3..R4 aside).
    let expected = |fcs: u16, fds: u16| {
        let mut image = vec![0xff; 28];
        image[0..2].copy_from_slice(&[0x7f, 0x02]);
        image[4..6].copy_from_slice(&[0x05, 0x38]);
        image[8..10].copy_from_slice(&[0xc8, 0x43]);
        image[12..16].copy_from_slice(&[0xef, 0xcd, 0xab, 0x89]);
        image[16..18].copy_from_slice(&fcs.to_le_bytes());
        image[18..20].copy_from_slice(&[0xaa, 0x05]);
        image[20..24].copy_from_slice(&[0x10, 0x32, 0x54, 0x76]);
        image[24..26].copy_from_slice(&fds.to_le_bytes());
        image
    };
    for (selectors, want) in [
        ((None, None), expected(0, 0)),
        ((Some(0x0033), Some(0x002b)), expected(0x33, 0x2b)),
        ((Some(0x1234), None), expected(0x1234, 0)),
        ((None, Some(0xfffe)), expected(0, 0xfffe)),
    ] {
        let mut x87 = X87State::fixture_seed();
        x87.instruction_selector = selectors.0;
        x87.data_selector = selectors.1;
        let before = x87.clone();
        let mut state = state_with(Some(x87), ENV);
        let mut ram = Ram::new(&FNSTENV);
        assert_eq!(run(&mut state, &mut ram).unwrap(), StepOutcome::Advanced);
        assert_eq!(
            &ram.bytes[ENV as usize..ENV as usize + 28],
            &want[..],
            "{selectors:?}"
        );
        assert_eq!(ram.writes, [(ENV, 28)]);
        // The environment forms are control instructions: FIP, FDP, FOP and the selectors stay.
        assert_eq!(state.x87.unwrap(), before);
        assert_eq!(state.rip, RIP + 2);
    }
}

#[test]
fn fnsave_stores_st_order_and_clears_the_known_selectors() {
    let mut x87 = X87State::fixture_seed();
    x87.status_word = (x87.status_word & !0x3800) | (3 << 11); // TOP = 3
    x87.instruction_selector = Some(0x33);
    let registers = x87.registers;
    let mut state = state_with(Some(x87), SAVE);
    let mut ram = Ram::new(&FNSAVE);
    assert_eq!(run(&mut state, &mut ram).unwrap(), StepOutcome::Advanced);
    for logical in 0..8 {
        assert_eq!(
            ram.bytes[SAVE as usize + 28 + 10 * logical..SAVE as usize + 38 + 10 * logical],
            registers[(3 + logical) & 7],
            "ST({logical}) is physical register {}",
            (3 + logical) & 7
        );
    }
    let x87 = state.x87.unwrap();
    assert_eq!(
        x87.instruction_selector,
        Some(0),
        "a known selector becomes 0"
    );
    assert_eq!(
        x87.data_selector, None,
        "an unrepresented selector stays unrepresented"
    );
    assert_eq!(
        x87.registers, registers,
        "the data registers survive like after FNINIT"
    );
}

#[test]
fn frstor_selector_loading_and_pointer_widths() {
    let mut image = vec![0u8; 108];
    image[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
    image[8..10].copy_from_slice(&0xffffu16.to_le_bytes());
    image[12..16].copy_from_slice(&0xdead_beefu32.to_le_bytes());
    image[18..20].copy_from_slice(&0xfaffu16.to_le_bytes()); // FOP bits 15:11 are not part of FOP
    image[20..24].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    for (before, fcs, fds, want) in [
        ((None, None), 0u16, 0u16, (None, None)),
        ((None, None), 0x33, 0, (Some(0x33), None)),
        ((None, None), 0, 0x2b, (None, Some(0x2b))),
        ((Some(7), Some(9)), 0, 0, (Some(0), Some(0))),
        ((Some(7), None), 0x40, 0, (Some(0x40), None)),
    ] {
        image[16..18].copy_from_slice(&fcs.to_le_bytes());
        image[24..26].copy_from_slice(&fds.to_le_bytes());
        let mut x87 = X87State::fixture_seed();
        x87.instruction_selector = before.0;
        x87.data_selector = before.1;
        let mut state = state_with(Some(x87), IN);
        let mut ram = Ram::new(&FRSTOR);
        ram.put(IN, &image);
        assert_eq!(run(&mut state, &mut ram).unwrap(), StepOutcome::Advanced);
        let x87 = state.x87.unwrap();
        assert_eq!(
            (x87.instruction_selector, x87.data_selector),
            want,
            "{before:?}"
        );
        // The upper 32 bits of FIP and FDP are cleared, FOP keeps 11 bits.
        assert_eq!(x87.instruction_pointer, 0xdead_beef);
        assert_eq!(x87.data_pointer, 0x1234_5678);
        assert_eq!(x87.last_opcode, 0x02ff);
        assert_eq!(
            (x87.control_word, x87.status_word, x87.tag_word),
            (0x037f, 0, 0xffff)
        );
        assert!(ram.writes.is_empty());
    }
}

#[test]
fn frstor_tags_follow_the_contents_and_empty_stays_empty() {
    let classes: [([u8; 10], X87Tag); 9] = [
        ([0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f], X87Tag::Valid),
        ([0; 10], X87Tag::Zero),
        ([0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80], X87Tag::Zero), // -0
        ([1, 0, 0, 0, 0, 0, 0, 0, 0, 0], X87Tag::Special), // denormal
        ([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0], X87Tag::Special), // pseudo-denormal
        ([0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f], X87Tag::Special), // infinity
        ([0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f], X87Tag::Special), // NaN
        ([0, 0, 0, 0, 0, 0, 0, 0x40, 0xff, 0x3f], X87Tag::Special), // unnormal
        ([1, 0, 0, 0, 0, 0, 0, 0, 0xff, 0x7f], X87Tag::Special), // pseudo-NaN
    ];
    let mut image = vec![0u8; 108];
    image[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
    image[4..6].copy_from_slice(&(2u16 << 11).to_le_bytes()); // TOP = 2
    // Every register is claimed Valid (tag 00) by the image, except R1 (tag 11 = Empty), which is ST(7).
    image[8..10].copy_from_slice(&0x000cu16.to_le_bytes());
    for (logical, (raw, _)) in classes.iter().take(8).enumerate() {
        image[28 + 10 * logical..38 + 10 * logical].copy_from_slice(raw);
    }
    let mut state = state_with(Some(X87State::fixture_seed()), IN);
    let mut ram = Ram::new(&FRSTOR);
    ram.put(IN, &image);
    assert_eq!(run(&mut state, &mut ram).unwrap(), StepOutcome::Advanced);
    let x87 = state.x87.as_ref().unwrap();
    for logical in 0..8 {
        let physical = ((2 + logical) & 7) as u8;
        let want = if logical == 7 {
            X87Tag::Empty // the tag word is by physical register; ST(7) with TOP = 2 is R1
        } else {
            classes[logical].1
        };
        assert_eq!(x87.tag(physical), Some(want), "ST({logical})");
    }
    // The ninth class goes through FNSTENV from a state that holds it in a non-empty register.
    let mut x87 = X87State::fixture_seed();
    x87.registers[0] = classes[8].0;
    x87.tag_word = 0;
    let mut state = state_with(Some(x87), ENV);
    let mut ram = Ram::new(&FNSTENV);
    assert_eq!(run(&mut state, &mut ram).unwrap(), StepOutcome::Advanced);
    assert_eq!(ram.bytes[ENV as usize + 8] & 3, X87Tag::Special as u8);
}

#[test]
fn refusals_leave_no_trace() {
    let valid_image = {
        let mut image = vec![0u8; 108];
        image[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
        image[8..10].copy_from_slice(&0xffffu16.to_le_bytes());
        image
    };
    let tweaks: Vec<(&str, Box<dyn Fn(&mut Option<X87State>)>)> = vec![
        ("no x87 state", Box::new(|x| *x = None)),
        (
            "control word with an unmasked exception",
            Box::new(|x| x.as_mut().unwrap().control_word = 0x037e),
        ),
        (
            "reserved control word bit 6 clear",
            Box::new(|x| x.as_mut().unwrap().control_word = 0x033f),
        ),
        (
            "ES set",
            Box::new(|x| x.as_mut().unwrap().status_word |= 0x0080),
        ),
        (
            "B set",
            Box::new(|x| x.as_mut().unwrap().status_word |= 0x8000),
        ),
    ];
    for code in [FNSTENV, FNSAVE, FRSTOR] {
        for (name, tweak) in &tweaks {
            let mut x87 = Some(X87State::fixture_seed());
            x87.as_mut().unwrap().control_word = 0x037f;
            x87.as_mut().unwrap().status_word = 0x3805;
            tweak(&mut x87);
            let mut state = state_with(x87, IN);
            let mut ram = Ram::new(&code);
            ram.put(IN, &valid_image);
            let before = (state.clone(), ram.bytes.clone());
            let outcome = run(&mut state, &mut ram).unwrap();
            // A state outside the profile is refused before any memory access, FRSTOR's read included.
            assert_eq!(
                outcome,
                StepOutcome::Unsupported { opcode: code[0] },
                "{code:02x?} {name}"
            );
            assert_eq!((state, ram.bytes), before, "{code:02x?} {name}");
            assert!(
                ram.writes.is_empty() && ram.reads.is_empty(),
                "{code:02x?} {name}"
            );
        }
    }
    // An image that FRSTOR cannot load is refused after the read, with nothing changed.
    let bad_images: Vec<(&str, Box<dyn Fn(&mut Vec<u8>)>)> = vec![
        (
            "control word with unmasked exception",
            Box::new(|i| i[0..2].copy_from_slice(&0x0360u16.to_le_bytes())),
        ),
        (
            "control word with X bit",
            Box::new(|i| i[0..2].copy_from_slice(&0x137fu16.to_le_bytes())),
        ),
        (
            "ES in the status word",
            Box::new(|i| i[4..6].copy_from_slice(&0x0080u16.to_le_bytes())),
        ),
        (
            "B in the status word",
            Box::new(|i| i[4..6].copy_from_slice(&0x8000u16.to_le_bytes())),
        ),
    ];
    for (name, tweak) in &bad_images {
        let mut image = valid_image.clone();
        tweak(&mut image);
        let mut state = state_with(
            Some({
                let mut x87 = X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = 0x3805;
                x87
            }),
            IN,
        );
        let mut ram = Ram::new(&FRSTOR);
        ram.put(IN, &image);
        let before = (state.clone(), ram.bytes.clone());
        assert_eq!(
            run(&mut state, &mut ram).unwrap(),
            StepOutcome::Unsupported { opcode: 0xdd },
            "{name}"
        );
        assert_eq!((state, ram.bytes), before, "{name}");
        assert_eq!(ram.reads, [(IN, 108)], "{name}");
    }
}

#[test]
fn memory_faults_leave_the_state_alone() {
    let mut x87 = X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0x3805;
    for code in [FNSTENV, FNSAVE, FRSTOR] {
        for address in [
            0x0000_8000_0000_0000u64, // non-canonical start
            0x0000_7fff_ffff_fff0,    // canonical start, the image crosses into the hole
            0xffff_7fff_ffff_ffff,    // non-canonical high half
        ] {
            let mut state = state_with(Some(x87.clone()), address);
            let mut ram = Ram::new(&code);
            let before = (state.clone(), ram.bytes.clone());
            let outcome = run(&mut state, &mut ram).unwrap();
            assert!(
                matches!(&outcome, StepOutcome::Fault(fault) if fault.vector == 13),
                "{code:02x?} {address:#x}: {outcome:?}"
            );
            assert_eq!((state, ram.bytes), before);
            assert!(ram.writes.is_empty() && ram.reads.is_empty());
        }
        // A canonical address outside the mapped memory: the access fails, the state is unchanged.
        let mut state = state_with(Some(x87.clone()), 0xfff0);
        let mut ram = Ram::new(&code);
        let before = (state.clone(), ram.bytes.clone());
        let outcome = run(&mut state, &mut ram);
        assert!(
            matches!(outcome, Err(MemoryError::Unmapped { .. })),
            "{code:02x?}: {outcome:?}"
        );
        assert_eq!((state, ram.bytes), before);
    }
}

#[test]
fn decoder_and_the_fwait_pairs() {
    let decode = |code: &[u8]| {
        let mut ram = Ram::new(code);
        long_decode::decode(&mut ram, RIP, AccessContext::default())
    };
    for (code, operation) in [
        (FNSTENV, Operation::FnstenvRax),
        (FNSAVE, Operation::FnsaveRax),
        (FRSTOR, Operation::FrstorRax),
    ] {
        let instruction = decode(&code).unwrap();
        assert_eq!((instruction.operation, instruction.length), (operation, 2));
        for prefix in [0x66u8, 0xf2, 0xf3, 0x2e, 0x67, 0x48, 0x64] {
            assert!(
                matches!(
                    decode(&[prefix, code[0], code[1]]),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{prefix:02x} {code:02x?}"
            );
        }
    }
    // FSTENV and FSAVE are FWAIT followed by the non-waiting form; FWAIT is the CPL0 set's, so the
    // bounded profile refuses the first byte and the pair is not a form of its own.
    let fwait = decode(&[0x9b, 0xd9, 0x30]).unwrap();
    assert_eq!((fwait.operation, fwait.length), (Operation::Fwait, 1));
    let mut state = state_with(Some(X87State::fixture_seed()), ENV);
    let mut ram = Ram::new(&[0x9b, 0xd9, 0x30]);
    assert_eq!(
        run(&mut state, &mut ram).unwrap(),
        StepOutcome::Unsupported { opcode: 0x9b }
    );
    // The neighbouring memory operands of the two groups are not these forms.
    for (opcode, modrm) in [
        (0xd9, 0x31),
        (0xd9, 0x32),
        (0xd9, 0x70),
        (0xd9, 0xb0),
        (0xdd, 0x21),
        (0xdd, 0x31),
        (0xdd, 0x60),
        (0xdd, 0xa0),
        (0xdd, 0x70),
        (0xdd, 0xb0),
    ] {
        let result = decode(&[opcode, modrm, 0, 0, 0, 0]);
        assert!(
            !matches!(
                result.as_ref().map(|i| i.operation),
                Ok(Operation::FnstenvRax | Operation::FnsaveRax | Operation::FrstorRax)
            ),
            "{opcode:02x} {modrm:02x}"
        );
    }
}
