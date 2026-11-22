//! S2-25o: the hooks of [`RealModeMemory`] that let a memory walk linear addresses itself
//! ([`RealModeMemory::walks_linear`]) and [`ProtectedCpu::step_admitting`].

use od_isa::legacy_mode::ProtectedSegment;
use od_isa::protected_exec::{CR4_PAE, ProtectedCpu, ProtectedInstruction, ProtectedRefusal};
use od_isa::real_mode_exec::{ImageMemory, ImageMemoryMut, RealModeMemory, TableRegister};
use std::cell::RefCell;

const CODE: u32 = 0x8000;
const CELL: u32 = 0x5010;

/// A memory that walks linear addresses itself: a flat 64 KiB image, and a log of what the model asked.
struct Linear {
    bytes: RefCell<Vec<u8>>,
    log: RefCell<Vec<String>>,
}

impl Linear {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![0u8; 0x1_0000];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        bytes[CELL as usize..CELL as usize + 4].copy_from_slice(&0x0BAD_F00Du32.to_le_bytes());
        Self {
            bytes: RefCell::new(bytes),
            log: RefCell::new(Vec::new()),
        }
    }
    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

impl RealModeMemory for Linear {
    fn walks_linear(&self) -> bool {
        true
    }
    fn fetch_u8(&self, linear: u32) -> Option<u8> {
        self.log.borrow_mut().push(format!("fetch {linear:#x}"));
        self.bytes.borrow().get(linear as usize).copied()
    }
    fn read_u8(&self, linear: u32) -> Option<u8> {
        self.log.borrow_mut().push(format!("read_u8 {linear:#x}"));
        self.bytes.borrow().get(linear as usize).copied()
    }
    fn can_write(&self, linear: u32) -> bool {
        self.log.borrow_mut().push(format!("can_write {linear:#x}"));
        false
    }
    fn write_u8(&mut self, linear: u32, _value: u8) -> bool {
        self.log.borrow_mut().push(format!("write_u8 {linear:#x}"));
        false
    }
    fn read_u32_linear(&self, linear: u32) -> Option<u32> {
        self.log.borrow_mut().push(format!("read_u32 {linear:#x}"));
        let bytes = self.bytes.borrow();
        let at = linear as usize;
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    }
    fn write_u32_linear(&mut self, linear: u32, value: u32) -> bool {
        self.log.borrow_mut().push(format!("write_u32 {linear:#x}"));
        let at = linear as usize;
        self.bytes.borrow_mut()[at..at + 4].copy_from_slice(&value.to_le_bytes());
        true
    }
}

fn segment(selector: u16, access: u8) -> ProtectedSegment {
    ProtectedSegment {
        selector,
        base: 0,
        limit: 0xFFFF_FFFF,
        access,
        flags: 0x0C,
    }
}

/// PAE paging on, with PDPTEs that would refuse every walk: a model that walked would fault.
fn cpu() -> ProtectedCpu {
    ProtectedCpu {
        gpr: [0; 8],
        eip: CODE,
        rip_high: 0,
        target_entered: false,
        eflags: 2,
        cr0: 0x8005_0033,
        cr3: 0x1000,
        cr4: CR4_PAE,
        efer: 0,
        pdptes: [0; 4],
        pse36: false,
        cs: segment(0x08, 0x9B),
        ds: segment(0x10, 0x93),
        es: segment(0x10, 0x93),
        ss: segment(0x10, 0x93),
        fs: segment(0x10, 0x93),
        gs: segment(0x10, 0x93),
        gdtr: TableRegister { base: 0, limit: 0 },
        idtr: TableRegister { base: 0, limit: 0 },
        mov_ss_shadow: false,
    }
}

fn disp(opcode: &[u8], value: u32) -> Vec<u8> {
    [opcode, &value.to_le_bytes()].concat()
}

#[test]
fn a_memory_that_walks_linear_addresses_is_not_walked_by_the_model_and_gets_one_operand_access() {
    let mut memory = Linear::new(&disp(&[0xA1], CELL));
    let mut cpu = cpu();
    assert_eq!(
        cpu.step(&mut memory),
        Ok(ProtectedInstruction::MovEaxFromDs(CELL))
    );
    assert_eq!(cpu.gpr[0], 0x0BAD_F00D);
    assert_eq!(cpu.eip, CODE + 5);
    // Five fetches in order, then the one DWORD read: no table was read, no byte was read one by one.
    let mut expected: Vec<String> = (0..5).map(|i| format!("fetch {:#x}", CODE + i)).collect();
    expected.push(format!("read_u32 {CELL:#x}"));
    assert_eq!(memory.log(), expected);

    // The store is one DWORD write; the segment checks stay the model's.
    let mut memory = Linear::new(&disp(&[0xA3], CELL));
    let mut cpu = self::cpu();
    cpu.gpr[0] = 0x1122_3344;
    assert_eq!(
        cpu.step(&mut memory),
        Ok(ProtectedInstruction::MovDsFromEax(CELL))
    );
    assert_eq!(
        memory.log().last().unwrap(),
        &format!("write_u32 {CELL:#x}")
    );
    assert_eq!(
        memory.bytes.borrow()[CELL as usize..CELL as usize + 4],
        0x1122_3344u32.to_le_bytes()
    );

    let mut memory = Linear::new(&disp(&[0xA3], CELL));
    let mut cpu = self::cpu();
    cpu.ds.access = 0x91; // not writable
    assert!(matches!(
        cpu.step(&mut memory),
        Err(ProtectedRefusal::DataSegmentReadOnly { access: 0x91, .. })
    ));
    assert!(memory.log().iter().all(|entry| entry.starts_with("fetch")));
    let mut memory = Linear::new(&disp(&[0xA1], CELL));
    let mut cpu = self::cpu();
    cpu.ds.limit = CELL + 2;
    assert!(matches!(
        cpu.step(&mut memory),
        Err(ProtectedRefusal::DataBeyondLimit { disp: CELL, .. })
    ));
}

#[test]
fn a_refused_operand_of_a_linear_memory_is_the_refusal_with_the_address() {
    let mut memory = Linear::new(&disp(&[0xA1], 0xFFFF_FF00));
    let mut cpu = cpu();
    let before = cpu;
    assert_eq!(
        cpu.step(&mut memory),
        Err(ProtectedRefusal::DataUnreadable {
            eip: CODE,
            linear: 0xFFFF_FF00
        })
    );
    assert_eq!(cpu, before);
}

#[test]
fn step_admitting_names_an_instruction_the_caller_does_not_admit_and_executes_nothing() {
    let mut memory = Linear::new(&disp(&[0xA3], CELL));
    let mut cpu = cpu();
    cpu.gpr[0] = 0xCAFE_0001;
    let before = cpu;
    let admit = |instruction: &ProtectedInstruction| {
        !matches!(instruction, ProtectedInstruction::MovDsFromEax(_))
    };
    assert_eq!(
        cpu.step_admitting(&mut memory, &admit),
        Err(ProtectedRefusal::InstructionNotAdmitted {
            eip: CODE,
            instruction: ProtectedInstruction::MovDsFromEax(CELL),
            length: 5,
        })
    );
    assert_eq!(cpu, before);
    assert!(
        memory.log().iter().all(|entry| entry.starts_with("fetch")),
        "{:?}",
        memory.log()
    );
    assert_eq!(
        memory.bytes.borrow()[CELL as usize..CELL as usize + 4],
        0x0BAD_F00Du32.to_le_bytes(),
        "nothing was stored"
    );
    // The same instruction, admitted, is what step does.
    let mut same = before;
    assert_eq!(
        same.step_admitting(&mut memory, &|_| true),
        Ok(ProtectedInstruction::MovDsFromEax(CELL))
    );
}

#[test]
fn the_defaults_are_physical_memory_with_the_bytewise_operands_of_before() {
    let image = [0x11u8, 0x22, 0x33, 0x44, 0x55];
    let rom = ImageMemory {
        base: 0x100,
        bytes: &image,
    };
    assert!(!rom.walks_linear());
    assert_eq!(rom.fetch_u8(0x101), Some(0x22));
    assert_eq!(rom.read_u32_linear(0x100), Some(0x4433_2211));
    assert_eq!(
        rom.read_u32_linear(0x102),
        None,
        "the fourth byte is outside the image"
    );

    let mut bytes = [0u8; 4];
    let mut ram = ImageMemoryMut {
        base: 0x100,
        bytes: &mut bytes,
    };
    assert!(ram.write_u32_linear(0x100, 0xAABB_CCDD));
    assert_eq!(ram.bytes, [0xDD, 0xCC, 0xBB, 0xAA]);
    assert!(
        !ram.write_u32_linear(0x102, 0),
        "a store that leaves the image changes nothing"
    );
    assert_eq!(ram.bytes, [0xDD, 0xCC, 0xBB, 0xAA]);
}
