//! S2-32d: FLDENV (bare `D9 20` with the 28-byte environment image at [RAX], no prefix) in the bounded local x87
//! profile, the inverse of FNSTENV (S2-32c).
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200): Vol. 2A "FLDENV" (loads the
//! environment from memory, the register stack contents are not touched), Vol. 1 section 8.1.8 (FLDENV loads the low 32
//! bits of FIP and FDP and clears the upper 32; FCS and FDS are loaded when CR0.PE = 1) and section 8.1.10 with Figure 8-9
//! (the 32-bit protected-mode image). Admission is the one of FRSTOR (S2-32c): a loaded control word from the twelve
//! all-exception-masked ones and neither ES nor B in the loaded status word (the SDM would deliver #MF at the next
//! waiting instruction for an unmasked exception, which this profile does not model), and the same condition on the
//! current state. CR0.EM/TS (#NM) is not modeled; CPL3 #AC was not modeled until ADR-0023 C1 (the area is checked on 4 bytes since).
//!
//! Policy for FCS and FDS is that of S2-32c: a loaded selector is `Some(v)`, except that a zero selector stays unrepresented
//! when it was. The tag word of the image gives empty or non-empty per register; a non-empty one is tagged from the register
//! contents already in the stack (Valid, Zero or Special), as FRSTOR does. The physical oracle is the Core i5-3210M
//! (`data/x87_fldenv_oracle.c`, rows from `data/x87_fldenv_oracle_gen.py`): each row of
//! `data/x87_fldenv_oracle_i5_3210m.txt` is `FNINIT; FRSTOR a; FLDENV b; FNSTENV env; FNSAVE save; FNSTENV env2` as
//! `a b env save env2` in hex, every byte of the three outputs compared.

use od_isa::execute::{StepOutcome, step};
use od_isa::long_decode::{self, DecodeError, Operation};
use od_isa::state::{X87State, X87Tag};
use od_isa::{AccessContext, AccessKind, GuestMemory, LongModeState, MemoryError, Register};

const ORACLE: &str = include_str!("data/x87_fldenv_oracle_i5_3210m.txt");
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

const FLDENV: [u8; 2] = [0xd9, 0x20];
const FNOP: [u8; 2] = [0xd9, 0xd0];
const NEWENV: u64 = 0x8000;

/// A 28-byte protected-mode environment image with FFFFH reserved fields.
#[allow(clippy::too_many_arguments)]
fn env_image(
    cw: u16,
    sw: u16,
    tw: u16,
    fip: u32,
    fcs: u16,
    fop: u16,
    fdp: u32,
    fds: u16,
) -> [u8; 28] {
    let mut image = [0xff_u8; 28];
    image[0..2].copy_from_slice(&cw.to_le_bytes());
    image[4..6].copy_from_slice(&sw.to_le_bytes());
    image[8..10].copy_from_slice(&tw.to_le_bytes());
    image[12..16].copy_from_slice(&fip.to_le_bytes());
    image[16..18].copy_from_slice(&fcs.to_le_bytes());
    image[18..20].copy_from_slice(&fop.to_le_bytes());
    image[20..24].copy_from_slice(&fdp.to_le_bytes());
    image[24..26].copy_from_slice(&fds.to_le_bytes());
    image
}

fn fixture() -> X87State {
    let mut x87 = X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0x3805;
    x87
}

#[test]
fn oracle_rows_are_reproduced_byte_for_byte() {
    let (mut rows, mut zero_fcs, mut zero_fds, mut empty_tags, mut top_moved) = (0, 0, 0, 0, 0);
    for line in ORACLE.lines() {
        let fields: Vec<Vec<u8>> = line.split_whitespace().map(hex).collect();
        let [a, b, env, save, env2] = &fields[..] else {
            panic!("{line}")
        };
        assert_eq!(
            (a.len(), b.len(), env.len(), save.len(), env2.len()),
            (108, 28, 28, 108, 28)
        );
        rows += 1;
        zero_fcs += usize::from(b[16..18] == [0, 0]);
        zero_fds += usize::from(b[24..26] == [0, 0]);
        empty_tags += usize::from(b[8..10] == [0xff, 0xff]);
        top_moved += usize::from(a[4..6] != b[4..6] && (a[5] ^ b[5]) & 0x38 != 0);
        // The starting selector state must not matter: the FLDENV image overwrites the whole environment.
        for selectors in [(None, None), (Some(0x5a5a), Some(0xa5a5))] {
            let mut x87 = X87State::fixture_seed();
            x87.instruction_selector = selectors.0;
            x87.data_selector = selectors.1;
            let mut state = state_with(Some(x87), IN);
            let mut ram = Ram::new(&[]);
            ram.put(IN, a);
            ram.put(NEWENV, b);
            assert_eq!(
                exec(&mut state, &mut ram, FRSTOR, IN),
                StepOutcome::Advanced,
                "{line}"
            );
            assert_eq!(
                exec(&mut state, &mut ram, FLDENV, NEWENV),
                StepOutcome::Advanced,
                "{line}"
            );
            assert_eq!(ram.reads, [(IN, 108), (NEWENV, 28)], "{line}");
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
        }
    }
    // The rows cover zero and non-zero selectors, empty-tag images and a TOP that FLDENV changes.
    assert_eq!(
        (rows, zero_fcs, zero_fds, empty_tags, top_moved),
        (324, 64, 80, 43, 284)
    );
}

#[test]
fn fldenv_loads_the_environment_and_leaves_the_registers() {
    let before = fixture();
    let mut state = state_with(Some(before.clone()), NEWENV);
    let mut ram = Ram::new(&[]);
    ram.put(
        NEWENV,
        &env_image(
            0x0c7f,
            0x2920,
            0xffff,
            0xdead_beef,
            0x1234,
            0xffff,
            0x89ab_cdef,
            0,
        ),
    );
    assert_eq!(
        exec(&mut state, &mut ram, FLDENV, NEWENV),
        StepOutcome::Advanced
    );
    assert_eq!(ram.reads, [(NEWENV, 28)], "one 28-byte read");
    assert!(ram.writes.is_empty());
    assert_eq!(state.rip, RIP + 2);
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!((x87.control_word, x87.status_word), (0x0c7f, 0x2920));
    assert_eq!(x87.top(), 5);
    // FIP and FDP are the low 32 bits only (the upper 32 are cleared), FOP keeps 11 bits.
    assert_eq!(
        (x87.instruction_pointer, x87.data_pointer, x87.last_opcode),
        (0xdead_beef, 0x89ab_cdef, 0x07ff)
    );
    // A non-zero selector is represented, a zero one stays unrepresented when it was.
    assert_eq!(
        (x87.instruction_selector, x87.data_selector),
        (Some(0x1234), None)
    );
    assert_eq!(x87.tag_word, 0xffff);
    assert_eq!(
        x87.registers, before.registers,
        "FLDENV leaves the registers"
    );

    // A zero selector over a represented one stays represented, as Some(0).
    let mut x87 = fixture();
    x87.instruction_selector = Some(0x5a5a);
    x87.data_selector = Some(0xa5a5);
    let mut state = state_with(Some(x87), NEWENV);
    ram.put(
        NEWENV,
        &env_image(0x037f, 0x0000, 0xffff, 1, 0, 2, 3, 0x0010),
    );
    assert_eq!(
        exec(&mut state, &mut ram, FLDENV, NEWENV),
        StepOutcome::Advanced
    );
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!(
        (x87.instruction_selector, x87.data_selector),
        (Some(0), Some(0x0010))
    );
    // While a selector is represented the other bounded forms refuse, FLDENV itself is exempt.
    assert_eq!(
        exec(&mut state, &mut ram, FNOP, 0),
        StepOutcome::Unsupported { opcode: 0xd9 }
    );
    assert_eq!(
        exec(&mut state, &mut ram, FLDENV, NEWENV),
        StepOutcome::Advanced
    );
}

#[test]
fn fldenv_tags_follow_the_contents_and_empty_stays_empty() {
    const ONE: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    const ZERO: [u8; 10] = [0; 10];
    const NAN: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f];
    const DENORMAL: [u8; 10] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut x87 = fixture();
    x87.registers = [ONE, ZERO, NAN, DENORMAL, ONE, ONE, ONE, ONE];
    for physical in 0..8 {
        x87.set_tag(physical, X87Tag::Valid);
    }
    x87.set_tag(5, X87Tag::Empty);
    let registers = x87.registers;
    let mut state = state_with(Some(x87), NEWENV);
    let mut ram = Ram::new(&[]);
    // Image tags by physical register: empty, valid, valid, zero, special, valid, empty, empty.
    let tags = 0b11_11_00_10_01_00_00_11_u16;
    ram.put(NEWENV, &env_image(0x037f, 0x0000, tags, 0, 0, 0, 0, 0));
    assert_eq!(
        exec(&mut state, &mut ram, FLDENV, NEWENV),
        StepOutcome::Advanced
    );
    let x87 = state.x87.as_ref().unwrap();
    let expected = [
        X87Tag::Empty,
        X87Tag::Zero,    // image says valid, the register holds zero
        X87Tag::Special, // image says valid, the register holds a NaN
        X87Tag::Special, // image says zero, the register holds a denormal
        X87Tag::Valid,   // image says special, the register holds 1.0
        X87Tag::Valid,   // was empty, the image says valid: tagged from the contents
        X87Tag::Empty,
        X87Tag::Empty,
    ];
    for (physical, tag) in expected.into_iter().enumerate() {
        assert_eq!(x87.tag(physical as u8), Some(tag), "physical {physical}");
    }
    assert_eq!(x87.registers, registers);
}

#[test]
fn fnstenv_then_fldenv_round_trips_the_environment() {
    let mut source = fixture();
    source.status_word = 0x2d21; // TOP 5, C0, C3 clear, PE and IE
    source.instruction_pointer = 0x1357_9bdf;
    source.data_pointer = 0x2468_ace0;
    source.last_opcode = 0x0123;
    source.instruction_selector = Some(0x0033);
    source.data_selector = Some(0x002b);
    let mut ram = Ram::new(&[]);
    let mut state = state_with(Some(source), ENV);
    assert_eq!(
        exec(&mut state, &mut ram, FNSTENV, ENV),
        StepOutcome::Advanced
    );
    let image = ram.bytes[ENV as usize..ENV as usize + 28].to_vec();
    for selectors in [(None, None), (Some(0x7777), Some(0x8888))] {
        let mut target = fixture();
        target.control_word = 0x0c7f;
        target.status_word = 0x0000;
        target.instruction_selector = selectors.0;
        target.data_selector = selectors.1;
        let mut state = state_with(Some(target), ENV);
        assert_eq!(
            exec(&mut state, &mut ram, FLDENV, ENV),
            StepOutcome::Advanced
        );
        assert_eq!(
            exec(&mut state, &mut ram, FNSTENV, ENV2),
            StepOutcome::Advanced
        );
        assert_eq!(&ram.bytes[ENV2 as usize..ENV2 as usize + 28], &image[..]);
    }
}

#[test]
fn refusals_leave_no_trace() {
    let valid_image = env_image(0x037f, 0x0000, 0xffff, 0, 0, 0, 0, 0);
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
    // A current state outside the profile is refused before any memory access.
    for (name, tweak) in &tweaks {
        let mut x87 = Some(fixture());
        tweak(&mut x87);
        let mut state = state_with(x87, IN);
        let mut ram = Ram::new(&FLDENV);
        ram.put(IN, &valid_image);
        let before = (state.clone(), ram.bytes.clone());
        assert_eq!(
            run(&mut state, &mut ram).unwrap(),
            StepOutcome::Unsupported { opcode: 0xd9 },
            "{name}"
        );
        assert_eq!((state, ram.bytes), before, "{name}");
        assert!(ram.writes.is_empty() && ram.reads.is_empty(), "{name}");
    }
    // An image that FLDENV cannot load is refused after the read, with nothing changed.
    let bad_images: Vec<(&str, Box<dyn Fn(&mut [u8; 28])>)> = vec![
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
        let mut image = valid_image;
        tweak(&mut image);
        let mut state = state_with(Some(fixture()), IN);
        let mut ram = Ram::new(&FLDENV);
        ram.put(IN, &image);
        let before = (state.clone(), ram.bytes.clone());
        assert_eq!(
            run(&mut state, &mut ram).unwrap(),
            StepOutcome::Unsupported { opcode: 0xd9 },
            "{name}"
        );
        assert_eq!((state, ram.bytes), before, "{name}");
        assert_eq!(ram.reads, [(IN, 28)], "{name}");
    }
}

#[test]
fn memory_faults_leave_the_state_alone() {
    for address in [
        0x0000_8000_0000_0000u64, // non-canonical start
        0x0000_7fff_ffff_ffe5,    // canonical start, the 28 bytes end in the hole
        0xffff_7fff_ffff_ffff,    // non-canonical high half
    ] {
        let mut state = state_with(Some(fixture()), address);
        let mut ram = Ram::new(&FLDENV);
        let before = (state.clone(), ram.bytes.clone());
        let outcome = run(&mut state, &mut ram).unwrap();
        assert!(
            matches!(&outcome, StepOutcome::Fault(fault) if fault.vector == 13),
            "{address:#x}: {outcome:?}"
        );
        assert_eq!((state, ram.bytes), before);
        assert!(ram.writes.is_empty() && ram.reads.is_empty());
    }
    // A canonical address outside the mapped memory: the access fails, the state is unchanged.
    let mut state = state_with(Some(fixture()), 0xfff0);
    let mut ram = Ram::new(&FLDENV);
    let before = (state.clone(), ram.bytes.clone());
    let outcome = run(&mut state, &mut ram);
    assert!(
        matches!(outcome, Err(MemoryError::Unmapped { .. })),
        "{outcome:?}"
    );
    assert_eq!((state, ram.bytes), before);
}

#[test]
fn an_address_whose_28_bytes_wrap_past_u64_max_is_an_address_overflow() {
    // Same behaviour as FRSTOR: the end address is computed with checked arithmetic before the
    // canonical check, so a start above u64::MAX - 27 is the model's AddressOverflow error (not #GP)
    // and nothing is read or changed (u64::MAX - 27 itself does not wrap: it is canonical, so the read
    // goes to memory).
    for address in [u64::MAX - 26, u64::MAX - 1, u64::MAX] {
        let mut state = state_with(Some(fixture()), address);
        let mut ram = Ram::new(&FLDENV);
        let before = (state.clone(), ram.bytes.clone());
        let outcome = run(&mut state, &mut ram);
        assert!(
            matches!(
                outcome,
                Err(MemoryError::AddressOverflow { address: a, length: 28 }) if a == address
            ),
            "{address:#x}: {outcome:?}"
        );
        assert_eq!((state, ram.bytes), before);
        assert!(ram.writes.is_empty() && ram.reads.is_empty());
    }
}

#[test]
fn decoder_and_the_fwait_pair() {
    let decode = |code: &[u8]| {
        let mut ram = Ram::new(code);
        long_decode::decode(&mut ram, RIP, AccessContext::default())
    };
    let instruction = decode(&FLDENV).unwrap();
    assert_eq!(
        (instruction.operation, instruction.length),
        (Operation::FldenvRax, 2)
    );
    for prefix in [0x66u8, 0xf2, 0xf3, 0x2e, 0x67, 0x48, 0x64] {
        assert!(
            matches!(
                decode(&[prefix, 0xd9, 0x20]),
                Err(DecodeError::Unsupported { .. })
            ),
            "{prefix:02x}"
        );
    }
    // The neighbouring memory operands of group D9 /4 are not this form.
    for modrm in [0x21, 0x22, 0x60, 0xa0] {
        let result = decode(&[0xd9, modrm, 0, 0, 0, 0]);
        assert!(
            !matches!(
                result.as_ref().map(|i| i.operation),
                Ok(Operation::FldenvRax)
            ),
            "d9 {modrm:02x}"
        );
    }
    // FWAIT is not a prefix of the bounded profile: the pair is not a form of its own.
    let mut state = state_with(Some(fixture()), ENV);
    let mut ram = Ram::new(&[0x9b, 0xd9, 0x20]);
    assert_eq!(
        run(&mut state, &mut ram).unwrap(),
        StepOutcome::Unsupported { opcode: 0x9b }
    );
}
