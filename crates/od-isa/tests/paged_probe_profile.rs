use std::collections::HashMap;

use od_isa::execute::{
    self, Cr0Profile, Cr4Profile, PagedCpl0Cr0WpProbeV1, PagedCpl0Cr3ProbeV1, PagedCpl0ProbeV1,
    PagedCpl0StackProbeV1, PagedProbeExit, PagedProbeRejection,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, Register, StiShadow,
};

const RIP: u64 = 0x4000;
const DATA: u64 = 0x8000;
const CODE_PHYSICAL: usize = 0x1000;
const DATA_PHYSICAL: usize = 0x3000;
const CR0: u64 = Cr0Profile::BASE;
const CR4: u64 = Cr4Profile::BASE;
const DEFINED: u64 = od_isa::INTEGER_FLAGS & !(1 << 4);

struct Memory {
    bytes: Vec<u8>,
    pages: HashMap<u64, usize>,
    fetches: Vec<(u64, AccessContext)>,
    reads: usize,
    writes: usize,
    fail_fetch: Option<MemoryError>,
    fail_read: Option<MemoryError>,
    fail_write: Option<MemoryError>,
}

impl Memory {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![0; 0x5000];
        bytes[CODE_PHYSICAL..CODE_PHYSICAL + code.len()].copy_from_slice(code);
        let mut pages = HashMap::new();
        pages.insert(RIP, CODE_PHYSICAL);
        pages.insert(DATA, DATA_PHYSICAL);
        Self {
            bytes,
            pages,
            fetches: Vec::new(),
            reads: 0,
            writes: 0,
            fail_fetch: None,
            fail_read: None,
            fail_write: None,
        }
    }

    fn physical(&self, address: u64) -> Result<usize, MemoryError> {
        let base = self
            .pages
            .get(&(address & !0xfff))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })?;
        Ok(base + (address as usize & 0xfff))
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetches.push((address, context));
        if let Some(error) = &self.fail_fetch {
            return Err(error.clone());
        }
        let physical = self.physical(address)?;
        self.bytes
            .get(physical)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.reads += 1;
        if let Some(error) = &self.fail_read {
            return Err(error.clone());
        }
        let physical = self.physical(address)?;
        let source = self
            .bytes
            .get(physical..physical + destination.len())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })?;
        destination.copy_from_slice(source);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        self.writes += 1;
        if let Some(error) = &self.fail_write {
            return Err(error.clone());
        }
        let physical = self.physical(address)?;
        let destination = self
            .bytes
            .get_mut(physical..physical + source.len())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })?;
        destination.copy_from_slice(source);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("probe MOV does not use atomic exchange")
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("probe MOV does not use atomic add")
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("probe MOV does not use atomic compare exchange")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!("probe does not issue a memory fence")
    }
}

#[derive(Default)]
struct Pio {
    input: u8,
    reads: Vec<u16>,
    writes: Vec<(u16, u8)>,
    fail_read: bool,
    fail_write: bool,
}

impl PortIo for Pio {
    type Error = &'static str;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        self.reads.push(port);
        if self.fail_read {
            Err("read")
        } else {
            Ok(self.input)
        }
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        self.writes.push((port, value));
        if self.fail_write {
            Err("write")
        } else {
            Ok(())
        }
    }
}

fn context(mode: InstructionMode, privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode,
        privilege,
        origin: AccessOrigin::DescriptorTable,
    }
}

fn state() -> LongModeState {
    LongModeState {
        cr3: None,
        rip: RIP,
        rflags: 0x8d7,
        cr0: Some(CR0),
        cr4: Some(CR4),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

fn step(
    state: &mut LongModeState,
    memory: &mut Memory,
    pio: &mut Pio,
    context: AccessContext,
) -> execute::PagedProbeResult<&'static str> {
    execute::step_paged_probe(PagedCpl0ProbeV1, state, memory, pio, context, DEFINED)
}

fn step_with_defined(
    state: &mut LongModeState,
    memory: &mut Memory,
    pio: &mut Pio,
    context: AccessContext,
    defined: u64,
) -> execute::PagedProbeResult<&'static str> {
    execute::step_paged_probe(PagedCpl0ProbeV1, state, memory, pio, context, defined)
}

fn page_fault(error_code: u32, address: u64) -> MemoryError {
    MemoryError::Architectural(od_isa::ArchitecturalFault {
        vector: 14,
        error_code: Some(error_code),
        linear_address: Some(address),
    })
}

#[test]
fn legacy_paged_probe_profile_keeps_stack_forms_unsupported() {
    for code in [[0x51], [0x58]] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr[Register::Rcx as usize] = 0x1122_3344_5566_7788;
        cpu.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0011;
        cpu.gpr[Register::Rsp as usize] = 0x2000;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );

        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: code[0] });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn paged_mov_load_uses_nonidentity_fetch_and_preserves_flags() {
    let mut memory = Memory::new(&[0x48, 0x8b, 0x03]);
    memory.bytes[DATA_PHYSICAL..DATA_PHYSICAL + 8]
        .copy_from_slice(&0x0123_4567_89ab_cdefu64.to_le_bytes());
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = DATA;
    let flags = cpu.rflags;
    let mut pio = Pio::default();

    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x0123_4567_89ab_cdef);
    assert_eq!(cpu.rip, RIP + 3);
    assert_eq!(cpu.rflags, flags);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(memory.fetches.len(), 3);
    assert!(memory.fetches.iter().all(|(_, context)| {
        context.origin == AccessOrigin::Explicit && context.privilege == PrivilegeLevel::Ring0
    }));
    assert_eq!(memory.reads, 1);
}

#[test]
fn paged_mov_store_commits_memory_then_retires_once() {
    let mut memory = Memory::new(&[0x48, 0x89, 0x0b]);
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = DATA;
    cpu.gpr[Register::Rcx as usize] = 0xfedc_ba98_7654_3210;
    let mut pio = Pio::default();

    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(cpu.rip, RIP + 3);
    assert_eq!(
        &memory.bytes[DATA_PHYSICAL..DATA_PHYSICAL + 8],
        &0xfedc_ba98_7654_3210u64.to_le_bytes()
    );
    assert_eq!(memory.writes, 1);
}

#[test]
fn pio_forms_use_one_callback_and_preserve_unrelated_cpu_state() {
    let mut memory = Memory::new(&[0xec]);
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.gpr[Register::Rdx as usize] = 0x1234_03f8;
    let flags = cpu.rflags;
    let mut pio = Pio {
        input: 0x5a,
        ..Pio::default()
    };

    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0xaabb_ccdd_eeff_005a);
    assert_eq!(cpu.gpr[Register::Rdx as usize], 0x1234_03f8);
    assert_eq!(cpu.rflags, flags);
    assert_eq!(pio.reads, [0x03f8]);
    assert_eq!(result.instructions_executed, 1);

    let mut memory = Memory::new(&[0xee]);
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 0x1234_5678_9abc_def0;
    cpu.gpr[Register::Rdx as usize] = 0x0000_0000_0000_03f8;
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(pio.writes, [(0x03f8, 0xf0)]);
    assert_eq!(cpu.rip, RIP + 1);
}

#[test]
fn hlt_retires_once_and_repeated_halted_boundary_has_no_effect() {
    let mut memory = Memory::new(&[0xf4]);
    let mut cpu = state();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Halted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cpu.activity, Some(CpuActivity::Halted));
    assert_eq!(cpu.rip, RIP + 1);
    let fetch_count = memory.fetches.len();
    let snapshot = cpu.clone();

    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Halted);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, snapshot);
    assert_eq!(memory.fetches.len(), fetch_count);
}

#[test]
fn admission_and_prefixed_forms_have_no_architectural_or_device_effects() {
    let mut memory = Memory::new(&[0xf4]);
    let mut pio = Pio::default();
    let mut cpu = state();
    let before = cpu.clone();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::WrongPrivilege)
    );
    assert_eq!(cpu, before);
    assert!(memory.fetches.is_empty());

    let mut memory = Memory::new(&[0xf3, 0xf4]);
    let mut cpu = state();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert!(matches!(result.exit, PagedProbeExit::Unsupported { .. }));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, state());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());

    let mut bad_cr = state();
    bad_cr.cr0 = Some(CR0 | 1 << 63);
    let mut memory = Memory::new(&[0xec]);
    let result = step(
        &mut bad_cr,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::ControlRegistersOutsideProfile)
    );
    assert!(memory.fetches.is_empty());
}

#[test]
fn fetch_data_and_pio_errors_do_not_retire_or_commit_cpu_state() {
    let mut memory = Memory::new(&[0x48, 0x8b, 0x03]);
    memory.fail_fetch = Some(MemoryError::Backend("fetch backend".into()));
    let mut cpu = state();
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Backend(MemoryError::Backend("fetch backend".into()))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0x48, 0x8b, 0x03]);
    memory.fail_read = Some(MemoryError::Backend("data backend".into()));
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = DATA;
    let before = cpu.clone();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Backend(MemoryError::Backend("data backend".into()))
    );
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0xee]);
    let mut cpu = state();
    cpu.gpr[Register::Rdx as usize] = 0x3f8;
    let before = cpu.clone();
    pio.fail_write = true;
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::PortIo("write"));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert_eq!(pio.writes, [(0x3f8, 0)]);
}

#[test]
fn admission_matrix_rejects_before_fetch_or_pio() {
    let mut cases = Vec::new();

    cases.push((
        state(),
        DEFINED | (1 << 12),
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::InvalidRflagsDefined {
            value: DEFINED | (1 << 12),
        }),
    ));

    let mut cpu = state();
    cpu.cr0 = None;
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::ControlRegistersMissing),
    ));

    let mut cpu = state();
    cpu.cr0 = Some(0);
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::ControlRegistersOutsideProfile),
    ));

    let mut cpu = state();
    cpu.cr4 = None;
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::ControlRegistersMissing),
    ));

    let mut cpu = state();
    cpu.cr4 = Some(0);
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::ControlRegistersOutsideProfile),
    ));

    let mut cpu = state();
    cpu.activity = None;
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::ActivityMissing),
    ));

    let mut cpu = state();
    cpu.activity = Some(CpuActivity::Halted);
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        None,
    ));

    let mut cpu = state();
    cpu.sti_shadow = Some(StiShadow::Blocking);
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::StiShadowActive),
    ));

    let mut cpu = state();
    cpu.sti_shadow = None;
    cases.push((
        cpu,
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        Some(PagedProbeRejection::StiShadowMissing),
    ));

    cases.push((
        state(),
        DEFINED,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        Some(PagedProbeRejection::WrongPrivilege),
    ));

    for (mut cpu, defined, context, expected_rejection) in cases {
        let mut memory = Memory::new(&[0xec]);
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, context, defined);
        if let Some(expected) = expected_rejection {
            assert_eq!(result.exit, PagedProbeExit::Rejected(expected));
        } else {
            assert_eq!(result.exit, PagedProbeExit::Halted);
        }
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, defined);
        assert_eq!(cpu, before);
        assert!(memory.fetches.is_empty());
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());
    }
}

#[test]
fn lock_bts_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7n belongs to the CPL0 set only; this profile refuses the form without mutation.
    for bytes in [
        &[0xf0, 0x48, 0x0f, 0xab, 0x03][..],
        &[0xf0, 0x48, 0x0f, 0xab, 0x05, 0x13, 0x7c, 0xf7, 0xff][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0xab },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn lock_btr_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7t belongs to the CPL0 set only; this profile refuses the form without mutation.
    for bytes in [
        &[0xf0, 0x48, 0x0f, 0xb3, 0x03][..],
        &[0xf0, 0x48, 0x0f, 0xb3, 0x05, 0x13, 0x7c, 0xf7, 0xff][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0xb3 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn lock_add_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7z belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for (bytes, opcode) in [
        (&[0xf0, 0x83, 0x44, 0x24, 0xfc, 0x00][..], 0x83),
        (&[0xf0, 0x83, 0x05, 0x13, 0x7c, 0xf7, 0xff, 0x01][..], 0x83),
        (&[0xf0, 0x81, 0x43, 0x58, 1, 0, 0, 0][..], 0x81),
        (&[0xf0, 0x48, 0x83, 0x43, 0x58, 0x01][..], 0x83),
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.gpr[Register::Rsp as usize] = DATA + 4;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn mov_sreg_store_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7u belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x8c, 0xd0][..],
        &[0x48, 0x8c, 0xd8][..],
        &[0x8c, 0x1b][..],
        &[0x65, 0x8c, 0x03][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.ss_selector = Some(0x20);
        cpu.cs_selector = Some(0x10);
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0x8c },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_test_byte_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7o belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0xf6, 0x05, 0x2c, 0x69, 0xf0, 0x02, 0xf0][..],
        &[0x64, 0xf6, 0x05, 0x34, 0x12, 0x00, 0x00, 0x0e][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0xf6 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_test_word_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7r belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x66, 0xf7, 0x05, 0x73, 0x68, 0xf0, 0x02, 0x01, 0xff][..],
        &[0x66, 0x64, 0xf7, 0x05, 0x34, 0x12, 0x00, 0x00, 0x0e, 0x00][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0xf7 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_inc_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7p belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0xff, 0x05, 0x73, 0x63, 0xf2, 0x02][..],
        &[0x64, 0xff, 0x05, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x48, 0xff, 0x43, 0x08][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0xff },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_dec_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7q belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0xff, 0x0d, 0x64, 0x63, 0xf2, 0x02][..],
        &[0x64, 0xff, 0x0d, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x48, 0xff, 0x4b, 0x08][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0xff },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn adjacent_mov_pio_and_hlt_encodings_are_outside_the_allowlist() {
    for bytes in [
        &[0x48, 0x8b, 0x0b][..],       // MOV RCX,[RBX]
        &[0x48, 0x89, 0x03][..],       // MOV [RBX],RAX
        &[0x4c, 0x8b, 0x03][..],       // MOV R8,[RBX]
        &[0x48, 0x8b, 0x43, 0x00][..], // MOV RAX,[RBX+0]
        &[0x8b, 0x03][..],             // MOV EAX,[RBX]
        &[0x66, 0xec][..],             // IN AX,DX
        &[0xf3, 0xec][..],             // REP IN
        &[0x40, 0xec][..],             // REX IN
        &[0xe4, 0x00][..],             // IN AL,imm8
        &[0xef][..],                   // OUT DX,EAX
        &[0x66, 0xf4][..],             // operand-size HLT prefix
        &[0xf3, 0xf4][..],             // REP HLT prefix
        &[0x40, 0xf4][..],             // REX HLT prefix
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert!(matches!(result.exit, PagedProbeExit::Unsupported { .. }));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());
    }
}

#[test]
fn fetch_faults_keep_literal_pfecs_and_do_not_call_pio() {
    for (error_code, address) in [(0x10, RIP), (0x11, RIP)] {
        let mut memory = Memory::new(&[0xec]);
        memory.fail_fetch = Some(page_fault(error_code, address));
        let mut cpu = state();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Fault(od_isa::ArchitecturalFault {
                vector: 14,
                error_code: Some(error_code),
                linear_address: Some(address),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());
    }
}

#[test]
fn data_faults_preserve_addresses_definedness_and_device_state() {
    let mut memory = Memory::new(&[0x48, 0x8b, 0x03]);
    memory.fail_read = Some(page_fault(0x0, DATA));
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());

    let mut memory = Memory::new(&[0x48, 0x89, 0x0b]);
    memory.fail_write = Some(page_fault(0x2, DATA));
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = DATA;
    cpu.gpr[Register::Rcx as usize] = 0xfeed_face_cafe_beef;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn in_pio_failure_preserves_cpu_definedness_and_retirement() {
    let mut memory = Memory::new(&[0xec]);
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.gpr[Register::Rdx as usize] = 0x03f8;
    let before = cpu.clone();
    let mut pio = Pio {
        fail_read: true,
        ..Pio::default()
    };

    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );

    assert_eq!(result.exit, PagedProbeExit::PortIo("read"));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(pio.reads, [0x03f8]);
    assert!(pio.writes.is_empty());
}

#[test]
fn store_backend_failure_preserves_cpu_definedness_and_retirement() {
    let mut memory = Memory::new(&[0x48, 0x89, 0x0b]);
    memory.fail_write = Some(MemoryError::Backend("store backend".into()));
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = DATA;
    cpu.gpr[Register::Rcx as usize] = 0xfeed_face_cafe_beef;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );

    assert_eq!(
        result.exit,
        PagedProbeExit::Backend(MemoryError::Backend("store backend".into()))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

fn gpr_patterns() -> [u64; 16] {
    let mut gpr = [0u64; 16];
    for (index, slot) in gpr.iter_mut().enumerate() {
        let n = index as u64 + 1;
        *slot = 0x1122_3344_5566_7788u64.rotate_left(n as u32 * 5) ^ (n * 0x0101_0101_0101_0101);
    }
    gpr
}

fn ring0() -> AccessContext {
    context(InstructionMode::Long64, PrivilegeLevel::Ring0)
}

#[test]
fn paged_probe_mov_r8_imm8_writes_only_the_target_byte() {
    for opcode in 0xb0u8..=0xb7 {
        let mut memory = Memory::new(&[opcode, 0x5a]);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !(1 << 6);

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired, "opcode {opcode:#x}");
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let r = usize::from(opcode & 7);
        let mut expected = before.clone();
        if r < 4 {
            expected.gpr[r] = (before.gpr[r] & !0xff) | 0x5a;
        } else {
            let base = r - 4;
            expected.gpr[base] = (before.gpr[base] & !0xff00) | (0x5a << 8);
        }
        expected.rip = RIP + 2;
        assert_eq!(cpu, expected, "opcode {opcode:#x}");
        assert_eq!(cpu.rflags, before.rflags);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn paged_probe_mov_r16_imm16_writes_only_the_low_word() {
    for opcode in 0xb8u8..=0xbf {
        let mut memory = Memory::new(&[0x66, opcode, 0xf8, 0x03]);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !(1 << 6);

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired, "opcode {opcode:#x}");
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let r = usize::from(opcode & 7);
        let mut expected = before.clone();
        expected.gpr[r] = (before.gpr[r] & !0xffff) | 0x03f8;
        expected.rip = RIP + 4;
        assert_eq!(cpu, expected, "opcode {opcode:#x}");
        assert_eq!(cpu.rflags, before.rflags);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn paged_probe_mov_immediate_neighbours_stay_unsupported() {
    // (bytes, expected Unsupported opcode). Results are the decoder's actual
    // classification; the decoder is not modified by S7-03.
    let cases: &[(&[u8], u8)] = &[
        (&[0xba, 0xf8, 0x03, 0x00, 0x00], 0xba), // MOV EDX,imm32 without 66
        (&[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8], 0xb8), // REX.W MOV RAX,imm64
        (&[0x41, 0xb0, 0x58], 0xb0),             // REX.B MOV R8B,imm8
        (&[0x40, 0xb4, 0x58], 0xb4),             // REX MOV SPL,imm8
        (&[0x66, 0xb0, 0x58], 0xb0),             // 66 B0
        (&[0x66, 0x66, 0xba, 0xf8, 0x03], 0xba), // repeated 66
        (&[0x66, 0x41, 0xba, 0xf8, 0x03], 0xba), // 66 + REX
        (&[0x67, 0xb0, 0x58], 0xb0),             // address override
        (&[0x2e, 0xb0, 0x58], 0xb0),             // segment override
        (&[0xf3, 0xb0, 0x58], 0xb0),             // F3
        (&[0xf2, 0xb0, 0x58], 0xb0),             // F2
        (&[0xc6, 0xc0, 0x58], 0xc6),             // MOV r/m8,imm8
        (&[0x66, 0xc7, 0xc2, 0xf8, 0x03], 0xc7), // MOV r/m16,imm16
        (&[0xc7, 0xc2, 0xf8, 0x03, 0x00, 0x00], 0xc7), // MOV r/m32,imm32
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(&mut cpu, &mut memory, &mut pio, ring0());
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: *opcode },
            "bytes {bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "bytes {bytes:02x?}");
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "bytes {bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn other_paged_profiles_keep_mov_immediate_unsupported() {
    use od_isa::execute::{PagedCpl0Cr0WpProbeV1, PagedCpl0Cr3ProbeV1, PagedCpl0StackProbeV1};
    for (bytes, opcode) in [
        (&[0xb0u8, 0x58][..], 0xb0u8),
        (&[0x66, 0xba, 0xf8, 0x03][..], 0xba),
    ] {
        for profile in ["stack", "cr3", "cr0wp"] {
            let mut memory = Memory::new(bytes);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            if profile != "stack" {
                cpu.cr3 = Some(0x1000);
            }
            let before = cpu.clone();
            let mut pio = Pio::default();
            let result = match profile {
                "stack" => execute::step_paged_stack_probe(
                    PagedCpl0StackProbeV1,
                    &mut cpu,
                    &mut memory,
                    &mut pio,
                    ring0(),
                    DEFINED,
                ),
                "cr3" => execute::step_paged_cr3_probe(
                    PagedCpl0Cr3ProbeV1,
                    &mut cpu,
                    &mut memory,
                    &mut pio,
                    ring0(),
                    0xd00,
                    DEFINED,
                ),
                _ => execute::step_paged_cr0_wp_probe(
                    PagedCpl0Cr0WpProbeV1,
                    &mut cpu,
                    &mut memory,
                    &mut pio,
                    ring0(),
                    0xd00,
                    DEFINED,
                ),
            };
            assert_eq!(
                result.exit,
                PagedProbeExit::Unsupported { opcode },
                "{profile} {bytes:02x?}"
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(cpu, before, "{profile} {bytes:02x?}");
        }
    }
}

fn rdtsc_ring0() -> AccessContext {
    context(InstructionMode::Long64, PrivilegeLevel::Ring0)
}

#[test]
fn paged_probe_rdtsc_with_source_splits_value_and_retires_once() {
    let mut memory = Memory::new(&[0x0f, 0x31]);
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 0xaaaa_bbbb_cccc_dddd;
    cpu.gpr[Register::Rdx as usize] = 0x1111_2222_3333_4444;
    let flags = cpu.rflags;
    let mut pio = Pio::default();
    let mut calls = 0;
    let mut source = || {
        calls += 1;
        0x1_2345_6789u64
    };

    let result = execute::step_paged_probe_with_rdtsc(
        PagedCpl0ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        rdtsc_ring0(),
        DEFINED,
        &mut source,
    );

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x2345_6789);
    assert_eq!(cpu.gpr[Register::Rdx as usize], 0x1);
    assert_eq!(cpu.rip, RIP + 2);
    assert_eq!(cpu.rflags, flags);
    assert_eq!(calls, 1);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn paged_probe_rdtsc_full_width_value_zero_extends_both_halves() {
    let mut memory = Memory::new(&[0x0f, 0x31]);
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = u64::MAX;
    cpu.gpr[Register::Rdx as usize] = u64::MAX;
    let mut pio = Pio::default();
    let mut source = || 0xfedc_ba98_7654_3210u64;

    let result = execute::step_paged_probe_with_rdtsc(
        PagedCpl0ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        rdtsc_ring0(),
        DEFINED,
        &mut source,
    );

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x7654_3210);
    assert_eq!(cpu.gpr[Register::Rdx as usize], 0xfedc_ba98);
}

#[test]
fn paged_probe_rdtsc_without_source_is_unsupported_and_pure() {
    let mut memory = Memory::new(&[0x0f, 0x31]);
    let mut cpu = state();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x31 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_rdtsc_prefixed_forms_do_not_call_source() {
    for code in [
        &[0x66u8, 0x0f, 0x31][..],
        &[0x48, 0x0f, 0x31][..],
        &[0xf3, 0x0f, 0x31][..],
        &[0xf2, 0x0f, 0x31][..],
    ] {
        let mut memory = Memory::new(code);
        let mut cpu = state();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let mut calls = 0;
        let mut source = || {
            calls += 1;
            0x1_2345_6789u64
        };

        let result = execute::step_paged_probe_with_rdtsc(
            PagedCpl0ProbeV1,
            &mut cpu,
            &mut memory,
            &mut pio,
            rdtsc_ring0(),
            DEFINED,
            &mut source,
        );

        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: 0x31 },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(calls, 0, "{code:02x?}");
    }
}

#[test]
fn paged_probe_rdtsc_entry_points_without_source_stay_unsupported() {
    // These entry points take no time source and pass None, so RDTSC is
    // unsupported by construction. This does not exercise the base_profile
    // gate; that is covered by a unit test in execute.rs.
    let mut memory = Memory::new(&[0x0f, 0x31]);
    let mut cpu = state();
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = execute::step_paged_stack_probe(
        PagedCpl0StackProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        rdtsc_ring0(),
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x31 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0x0f, 0x31]);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = execute::step_paged_cr3_probe(
        PagedCpl0Cr3ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        rdtsc_ring0(),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x31 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0x0f, 0x31]);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = execute::step_paged_cr0_wp_probe(
        PagedCpl0Cr0WpProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        rdtsc_ring0(),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x31 });
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_rdtsc_lock_form_is_ud_in_base_profile() {
    let mut memory = Memory::new(&[0xf0, 0x0f, 0x31]);
    let mut cpu = state();
    let before = cpu.clone();
    let mut pio = Pio::default();
    let mut calls = 0;
    let mut source = || {
        calls += 1;
        1u64
    };
    let result = execute::step_paged_probe_with_rdtsc(
        PagedCpl0ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        rdtsc_ring0(),
        DEFINED,
        &mut source,
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 6,
            error_code: None,
            linear_address: None,
        })
    );
    assert_eq!(cpu, before);
    assert_eq!(calls, 0);
}
