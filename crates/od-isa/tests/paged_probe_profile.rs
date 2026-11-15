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
    fail_fetch_at: Option<(u64, MemoryError)>,
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
            fail_fetch_at: None,
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
        if let Some((fail_address, error)) = &self.fail_fetch_at {
            if address == *fail_address {
                return Err(error.clone());
            }
        }
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
fn lock_add_register_source_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ai belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0xf0, 0x48, 0x01, 0x3d, 0x8f, 0x02, 0xdc, 0x02][..],
        &[0xf0, 0x48, 0x01, 0x43, 0x58][..],
        &[0xf0, 0x01, 0x43, 0x58][..],
        &[0xf0, 0x4c, 0x01, 0x4b, 0x58][..],
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
            PagedProbeExit::Unsupported { opcode: 0x01 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn lock_dword_xadd_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ay belongs to the CPL0 set only; this profile refuses the LOCK dword form (and the
    // register form with LOCK) without mutation. The S2-21f qword form is unchanged.
    for bytes in [
        &[0xf0, 0x0f, 0xc1, 0x43, 0x58][..],
        &[0xf0, 0x44, 0x0f, 0xc1, 0x05, 0x34, 0x12, 0x00, 0x00][..],
        &[0xf0, 0x48, 0x0f, 0xc1, 0xc3][..],
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
            PagedProbeExit::Unsupported { opcode: 0xc1 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn lock_and_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ag belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0xf0, 0x80, 0x25, 0x6f, 0x39, 0xf8, 0xff, 0xfe][..],
        &[0xf0, 0x80, 0x63, 0x58, 0xfe][..],
        &[0xf0, 0x41, 0x80, 0x61, 0x00, 0xfe][..],
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
            PagedProbeExit::Unsupported { opcode: 0x80 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn lock_bts_imm_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bi belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0xf0, 0x49, 0x0f, 0xba, 0x2c, 0x24, 0x00][..],
        &[0xf0, 0x48, 0x0f, 0xba, 0x2b, 0x05][..],
        &[0xf0, 0x0f, 0xba, 0x2d, 0x34, 0x12, 0x00, 0x00, 0x1f][..],
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
            PagedProbeExit::Unsupported { opcode: 0xba },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_cmp_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7aa belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x48, 0x39, 0x15, 0x08, 0x8a, 0xce, 0x01][..],
        &[0x64, 0x48, 0x39, 0x15, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x39, 0x15, 0x34, 0x12, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0x39 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_or_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7az belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x09, 0x15, 0x68, 0xcf, 0xf0, 0x02][..],
        &[0x64, 0x48, 0x09, 0x15, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x4c, 0x09, 0x43, 0x08][..],
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
            PagedProbeExit::Unsupported { opcode: 0x09 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_add_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bc belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x48, 0x01, 0x05, 0x36, 0x26, 0xc0, 0x02][..],
        &[0x64, 0x48, 0x01, 0x15, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x4c, 0x01, 0x43, 0x08][..],
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
            PagedProbeExit::Unsupported { opcode: 0x01 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_add_load_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bd belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x48, 0x03, 0x1d, 0x7f, 0x61, 0xbe, 0x02][..],
        &[0x64, 0x48, 0x03, 0x15, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x03, 0x15, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x4c, 0x03, 0x43, 0x08][..],
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
            PagedProbeExit::Unsupported { opcode: 0x03 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_cmp_load_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7be belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x3b, 0x35, 0x22, 0xc5, 0xbf, 0x02][..],
        &[0x64, 0x48, 0x3b, 0x15, 0x34, 0x12, 0x00, 0x00][..],
        &[0x65, 0x4c, 0x3b, 0x43, 0x08][..],
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
            PagedProbeExit::Unsupported { opcode: 0x3b },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_cmov_load_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bj belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for (bytes, opcode) in [
        (
            &[0x65, 0x0f, 0x44, 0x3d, 0x60, 0x95, 0x08, 0x03][..],
            0x44u8,
        ),
        (
            &[0x64, 0x48, 0x0f, 0x45, 0x15, 0x34, 0x12, 0x00, 0x00][..],
            0x45,
        ),
        (&[0x65, 0x4c, 0x0f, 0x4f, 0x43, 0x08][..], 0x4f),
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
            PagedProbeExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_cmp_imm8_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ah belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x83, 0x3d, 0x95, 0x79, 0xe9, 0x02, 0x01][..],
        &[0x64, 0x48, 0x83, 0x3d, 0x34, 0x12, 0x00, 0x00, 0xff][..],
        &[0x65, 0x83, 0x78, 0x08, 0x01][..],
        // S4-38L9j: the word forms (66 83 /7 ib, 66 81 /7 iw) are CPL0-set only as well.
        &[0x65, 0x66, 0x83, 0x3d, 0x35, 0xee, 0xfc, 0x02, 0x00][..],
        &[0x66, 0x64, 0x83, 0x78, 0x08, 0xff][..],
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
            PagedProbeExit::Unsupported { opcode: 0x83 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_81_family_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9l (AND/SUB/XOR/CMP with imm32 and FS/GS) belongs to the CPL0 set only; this profile
    // refuses the forms without mutation.
    for bytes in [
        &[
            0x65, 0x81, 0x25, 0xb9, 0x03, 0xf8, 0x02, 0xff, 0xff, 0xff, 0x7f,
        ][..],
        &[
            0x64, 0x48, 0x81, 0x2d, 0x34, 0x12, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
        ][..],
        &[0x65, 0x81, 0x70, 0x08, 0x01, 0x00, 0x00, 0x00][..],
        &[
            0x64, 0x48, 0x81, 0x3d, 0x34, 0x12, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ][..],
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
            PagedProbeExit::Unsupported { opcode: 0x81 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_80_family_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9m (byte ADD/AND/SUB/XOR/CMP with imm8 and FS/GS) belongs to the CPL0 set only; this
    // profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x80, 0x3d, 0xce, 0x8f, 0xef, 0x02, 0x00][..],
        &[0x64, 0x80, 0x2d, 0x34, 0x12, 0x00, 0x00, 0xff][..],
        &[0x65, 0x80, 0x70, 0x08, 0x01][..],
        &[0x64, 0x41, 0x80, 0x00, 0x01][..],
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
            PagedProbeExit::Unsupported { opcode: 0x80 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_add_imm32_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9f belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[
            0x65, 0x81, 0x05, 0x84, 0x37, 0xcf, 0x01, 0x00, 0x00, 0x11, 0x00,
        ][..],
        &[
            0x64, 0x48, 0x81, 0x05, 0x34, 0x12, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
        ][..],
        &[0x65, 0x81, 0x40, 0x08, 0x01, 0x00, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0x81 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn pause_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9b belongs to the CPL0 set only; this profile refuses `F3 90` without mutation.
    let mut memory = Memory::new(&[0xf3, 0x90]);
    let mut cpu = state();
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x90 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn invlpg_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ab belongs to the CPL0 set only; this profile refuses it without mutation.
    for bytes in [
        &[0x0f, 0x01, 0x3f][..],
        &[0x65, 0x0f, 0x01, 0x3f][..],
        &[0x0f, 0x01, 0x3d, 0x34, 0x12, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0x01 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn ltr_register_form_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7am belongs to the CPL0 set only; this profile refuses `LTR r/m16` without mutation
    // (the bounded paged LTR has its own entry point).
    for bytes in [
        &[0x0f, 0x00, 0xd8][..],
        &[0x41, 0x0f, 0x00, 0xdf][..],
        &[0x40, 0x0f, 0x00, 0xd8][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gdtr = Some(od_isa::state::Gdtr {
            base: 0xffff_fe00_0000_1000,
            limit: 0x7f,
        });
        cpu.gpr[Register::Rax as usize] = 0x40;
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
            PagedProbeExit::Unsupported { opcode: 0x00 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(cpu.tr.is_none());
    }
}

#[test]
fn lldt_register_form_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7aw belongs to the CPL0 set only; this profile refuses `LLDT r/m16` without mutation
    // (null selector included; the bounded paged LTR entry point does not take it either).
    for bytes in [
        &[0x0f, 0x00, 0xd6][..],
        &[0x41, 0x0f, 0x00, 0xd7][..],
        &[0x40, 0x0f, 0x00, 0xd0][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gdtr = Some(od_isa::state::Gdtr {
            base: 0xffff_fe00_0000_1000,
            limit: 0x7f,
        });
        cpu.gpr[Register::Rsi as usize] = 0;
        cpu.gpr[Register::Rax as usize] = 0;
        cpu.gpr[Register::Rdi as usize] = 0;
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
            PagedProbeExit::Unsupported { opcode: 0x00 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(cpu.ldtr_null_selector.is_none());
    }
}

#[test]
fn mov_drn_write_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ax belongs to the CPL0 set only; this profile refuses `MOV DRn, r64` without mutation.
    for bytes in [
        &[0x0f, 0x23, 0xfe][..],
        &[0x41, 0x0f, 0x23, 0xfe][..],
        &[0x48, 0x0f, 0x23, 0xc0][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rsi as usize] = 0x400;
        cpu.gpr[Register::Rax as usize] = 0x400;
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
            PagedProbeExit::Unsupported { opcode: 0x23 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert_eq!(cpu.debug_registers, [None; 8]);
    }
}

#[test]
fn sgdt_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7al belongs to the CPL0 set only; this profile refuses it without mutation.
    for bytes in [
        &[0x0f, 0x01, 0x45, 0xe6][..],
        &[0x65, 0x0f, 0x01, 0x45, 0xe6][..],
        &[0x0f, 0x01, 0x05, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gdtr = Some(od_isa::state::Gdtr {
            base: 0xffff_fe00_0000_1000,
            limit: 0x7f,
        });
        cpu.gpr[Register::Rbp as usize] = DATA;
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
            PagedProbeExit::Unsupported { opcode: 0x01 },
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
fn fs_gs_movzx_word_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ad belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x0f, 0xb7, 0x3d, 0x05, 0xa0, 0x01, 0x03][..],
        &[0x64, 0x48, 0x0f, 0xb7, 0x05, 0x34, 0x12, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0xb7 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_mov_byte_immediate_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ae belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0xc6, 0x05, 0x68, 0xa2, 0x01, 0x03, 0x01][..],
        &[0x64, 0x48, 0xc6, 0x05, 0x34, 0x12, 0x00, 0x00, 0xff][..],
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
            PagedProbeExit::Unsupported { opcode: 0xc6 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_mov_imm32_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7aj belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[
            0x65, 0xc7, 0x05, 0xca, 0xf3, 0x4c, 0x00, 0xec, 0x00, 0x00, 0x00,
        ][..],
        &[
            0x64, 0x48, 0xc7, 0x05, 0x34, 0x12, 0x00, 0x00, 0xfe, 0xff, 0xff, 0xff,
        ][..],
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
            PagedProbeExit::Unsupported { opcode: 0xc7 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_mov_store_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7an and S4-38L7av (and S4-38L7g) belong to the CPL0 set only; this profile refuses the forms
    // without mutation.
    for bytes in [
        &[0x65, 0x89, 0x05, 0xe0, 0x0f, 0x07, 0x03][..],
        &[0x64, 0x89, 0x03][..],
        &[0x65, 0x44, 0x89, 0x0b][..],
        &[0x65, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00][..],
        // S4-38L7av: the word forms (`66`, no REX.W).
        &[0x65, 0x66, 0x89, 0x05, 0xad, 0xaa, 0x01, 0x03][..],
        &[0x65, 0x66, 0x89, 0x15, 0xa7, 0xaa, 0x01, 0x03][..],
        &[0x64, 0x66, 0x44, 0x89, 0x0b][..],
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
            PagedProbeExit::Unsupported { opcode: 0x89 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_test_dword_qword_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ac belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[
            0x65, 0xf7, 0x05, 0xfc, 0xf8, 0xff, 0x02, 0xff, 0xff, 0xff, 0x7f,
        ][..],
        &[
            0x64, 0x48, 0xf7, 0x05, 0x34, 0x12, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
        ][..],
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
        &[0xfd][..],                   // STD
        &[0xf3, 0xfc][..],             // prefixed CLD
        &[0x66, 0xfc][..],             // operand-size prefixed CLD
        &[0x40, 0xfc][..],             // REX-prefixed CLD
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
fn bare_cld_clears_only_df_in_the_base_profile() {
    const DF: u64 = 1 << 10;

    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    cpu.rflags |= DF;
    let mut expected = cpu.clone();
    expected.rflags &= !DF;
    expected.rip += 1;
    let mut pio = Pio::default();
    let result = execute::step_paged_probe(
        PagedCpl0ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        DEFINED & !DF,
    );
    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty());
    assert!(pio.writes.is_empty());

    // The firmware's documented handoff already has DF=0; CLD still retires
    // without changing that value or any other state.
    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    let mut expected = cpu.clone();
    expected.rip += 1;
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, expected);

    // The explicit-RDTSC wrapper is the same base profile plus one operation.
    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    cpu.rflags |= DF;
    let mut pio = Pio::default();
    let mut source = || 0;
    let result = execute::step_paged_probe_with_rdtsc(
        PagedCpl0ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        DEFINED,
        &mut source,
    );
    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cpu.rflags & DF, 0);
}

#[test]
fn cld_fetch_fault_and_non_base_profiles_do_not_commit() {
    const DF: u64 = 1 << 10;

    let mut memory = Memory::new(&[0xfc]);
    memory.fail_fetch = Some(page_fault(0x10, RIP));
    let mut cpu = state();
    cpu.rflags |= DF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert!(matches!(result.exit, PagedProbeExit::Fault(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    cpu.rflags |= DF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = execute::step_paged_stack_probe(
        PagedCpl0StackProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xfc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.rflags |= DF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = execute::step_paged_cr3_probe(
        PagedCpl0Cr3ProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xfc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.rflags |= DF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = execute::step_paged_cr0_wp_probe(
        PagedCpl0Cr0WpProbeV1,
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xfc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&[0xfc]);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.rflags |= DF;
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xfc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);
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
        (&[0xc7, 0xc0, 0, 0, 0, 0], 0xc7),       // MOV EAX,imm32
        (&[0x48, 0xc7, 0xc1, 0, 0, 0, 0], 0xc7), // MOV RCX,imm32
        (&[0x49, 0xc7, 0xc0, 0, 0, 0, 0], 0xc7), // MOV R8,imm32
        (&[0x48, 0xc7, 0x00, 0, 0, 0, 0], 0xc7), // MOV [RAX],imm32
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
fn firmware_entry_c7_sets_only_rax_and_retires_once() {
    let bytes = [0x48, 0xc7, 0xc0, 0x00, 0x00, 0x10, 0x00];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.rflags |= 1 << 10;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::Rax as usize] = 0x0010_0000;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());

    // The same exact shape sign-extends the immediate as REX.W C7 /0 requires.
    let bytes = [0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, ring0());
    assert_eq!(result.exit, PagedProbeExit::Retired);
    let mut expected = before;
    expected.gpr[Register::Rax as usize] = u64::MAX;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
}

#[test]
fn firmware_entry_c7_fault_fetching_immediate_does_not_commit() {
    // Place the encoding so fetching its first immediate byte faults.
    let mut memory = Memory::new(&[]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffd;
    memory.bytes[CODE_PHYSICAL + 0xffd..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0xc7, 0xc0]);
    memory.fail_fetch_at = Some((RIP + 0x1000, page_fault(0x10, RIP + 0x1000)));
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(RIP + 0x1000),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(
        memory
            .fetches
            .iter()
            .any(|(address, _)| *address == RIP + 0x1000)
    );
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_cmp_fault_fetching_modrm_does_not_commit() {
    let mut memory = Memory::new(&[]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffe;
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x48, 0x39]);
    memory.fail_fetch_at = Some((RIP + 0x1000, page_fault(0x10, RIP + 0x1000)));
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(RIP + 0x1000),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(
        memory
            .fetches
            .iter()
            .any(|(address, _)| *address == RIP + 0x1000)
    );
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_cmp_rdi_rax_updates_only_arithmetic_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const ARITH_FLAGS: u64 = CF | PF | AF | ZF | SF | OF;
    let code = [0x48, 0x39, 0xc7]; // CMP RDI,RAX
    let cases = [
        (0x1234, 0x1234, PF | ZF),
        (0, 1, CF | PF | AF | SF),
        (i64::MAX as u64, u64::MAX, CF | PF | SF | OF),
    ];

    for (rdi, rax, cmp_flags) in cases {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rdi as usize] = rdi;
        cpu.gpr[Register::Rax as usize] = rax;
        cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | ARITH_FLAGS;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !(1 << 10);

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired, "{rdi:#x} - {rax:#x}");
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | ARITH_FLAGS);
        let mut expected = before;
        expected.rflags = (expected.rflags & !ARITH_FLAGS) | cmp_flags;
        expected.rip += code.len() as u64;
        assert_eq!(cpu, expected, "{rdi:#x} - {rax:#x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_cmp_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x39, 0xc7], 0x39),             // dword CMP EDI,EAX
        (&[0x48, 0x39, 0xc6], 0x39),       // CMP RSI,RAX
        (&[0x48, 0x3b, 0xc7], 0x3b),       // reverse direction: CMP RAX,RDI
        (&[0x48, 0x39, 0x07], 0x39),       // CMP [RDI],RAX
        (&[0x66, 0x48, 0x39, 0xc7], 0x39), // extra operand-size prefix
        (&[0x40, 0x48, 0x39, 0xc7], 0x39), // redundant REX
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
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "bytes {bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_cmp_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x39, 0xc7];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x39 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_jnz_taken_and_not_taken_preserve_flags_and_definedness() {
    let code = [0x75, 0x24]; // JNZ +0x24
    let defined = DEFINED & !(1 << 6);
    for (zero, expected_rip) in [(false, RIP + 2 + 0x24), (true, RIP + 2)] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | if zero { 1 << 6 } else { 0 };
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let mut expected = before;
        expected.rip = expected_rip;
        assert_eq!(cpu, expected);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_jnz_checks_canonicality_only_when_taken() {
    let code = [0x75, 0x24];
    let start = 0x0000_7fff_ffff_fffe;
    let mut memory = Memory::new(&[]);
    memory.pages.insert(start & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);

    let mut cpu = state();
    cpu.rip = start;
    cpu.rflags |= 1 << 6; // ZF=1: JNZ not taken; fall-through is non-canonical.
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.rip = 0x0000_8000_0000_0000;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());

    let mut memory = Memory::new(&[]);
    memory.pages.insert(start & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = start;
    cpu.rflags &= !(1 << 6); // ZF=0: branch target is non-canonical.
    let before = cpu.clone();
    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_jnz_fetch_fault_on_rel8_does_not_commit() {
    // Put the opcode at the last byte of a mapped code page so rel8 fetch faults.
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x75;
    let mut cpu = state();
    cpu.rip = RIP + 0xfff;
    cpu.gpr = gpr_patterns();
    cpu.rflags &= !(1 << 6);
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 1, page_fault(0x10, cpu.rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(RIP + 0x1000),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert!(
        memory
            .fetches
            .iter()
            .any(|(address, _)| *address == RIP + 0x1000)
    );
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_jnz_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x75, 0x23], 0x75),                // different displacement
        (&[0x74, 0x24], 0x74),                // JZ
        (&[0x0f, 0x85, 0x24, 0, 0, 0], 0x85), // near JNZ
        (&[0x66, 0x75, 0x24], 0x75),          // prefixed JNZ
        (&[0x40, 0x75, 0x24], 0x75),          // REX-prefixed JNZ
        (&[0xeb, 0x24], 0xeb),                // JMP rel8
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
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "bytes {bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_jnz_is_unsupported_in_cpl3_profile() {
    let bytes = [0x75, 0x24];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x75 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_mov_r8_rdi_copies_only_r8_and_advances_rip() {
    let code = [0x49, 0x89, 0xf8]; // MOV R8,RDI
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rdi as usize] = 0x1122_3344_5566_7788;
    cpu.gpr[Register::R8 as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::R8 as usize] = expected.gpr[Register::Rdi as usize];
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_mov_r8_rdi_fetch_fault_does_not_commit() {
    // Place the ModRM byte on the next unmapped code page.
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x49, 0x89]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffe;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 2, page_fault(0x10, cpu.rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(RIP + 0x1000),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert!(
        memory
            .fetches
            .iter()
            .any(|(address, _)| *address == RIP + 0x1000)
    );
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_mov_r8_rdi_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x49, 0x89, 0xf9], 0x89),       // MOV R9,RDI
        (&[0x49, 0x89, 0xf0], 0x89),       // MOV R8,RSI
        (&[0x49, 0x8b, 0xf8], 0x8b),       // reverse direction: MOV RDI,R8
        (&[0x41, 0x89, 0xf8], 0x89),       // dword MOV R8D,EDI
        (&[0x49, 0x89, 0x38], 0x89),       // MOV [R8],RDI
        (&[0x66, 0x49, 0x89, 0xf8], 0x89), // extra operand-size prefix
        (&[0x40, 0x49, 0x89, 0xf8], 0x89), // redundant REX
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
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "bytes {bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_mov_r8_rdi_is_unsupported_in_cpl3_profile() {
    let bytes = [0x49, 0x89, 0xf8];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x49 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_lea_rdi_rip_disp32_sets_exact_target_only() {
    let code = [0x48, 0x8d, 0x3d, 0x09, 0x2b, 0, 0];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.rip = RIP;
    cpu.gpr[Register::Rdi as usize] = 0x1122_3344_5566_7788;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::Rdi as usize] = RIP + code.len() as u64 + 0x2b09;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_lea_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffa..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x8d, 0x3d, 0x09, 0x2b, 0]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffa;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 6, page_fault(0x10, cpu.rip + 6)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(RIP + 0x1000),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert!(
        memory
            .fetches
            .iter()
            .any(|(address, _)| *address == RIP + 0x1000)
    );
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_lea_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x8d, 0x3d, 0x08, 0x2b, 0, 0], 0x8d), // displacement differs
        (&[0x48, 0x8d, 0x35, 0x09, 0x2b, 0, 0], 0x8d), // RSI destination
        (&[0x48, 0x8d, 0x05, 0x09, 0x2b, 0, 0], 0x8d), // RAX destination
        (&[0x40, 0x8d, 0x3d, 0x09, 0x2b, 0, 0], 0x8d), // no REX.W
        (&[0x48, 0x8d, 0x3f], 0x8d),                   // register base, not RIP-relative
        (&[0x67, 0x48, 0x8d, 0x3d, 0x09, 0x2b, 0, 0], 0x8d), // address override
        (&[0x66, 0x48, 0x8d, 0x3d, 0x09, 0x2b, 0, 0], 0x8d), // extra prefix
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
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "{bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_lea_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x8d, 0x3d, 0x09, 0x2b, 0, 0];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x8d });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn other_paged_profiles_keep_s7_entry_instructions_unsupported() {
    use od_isa::execute::{PagedCpl0Cr0WpProbeV1, PagedCpl0Cr3ProbeV1, PagedCpl0StackProbeV1};
    for (bytes, opcode) in [
        (&[0xb0u8, 0x58][..], 0xb0u8),
        (&[0x66, 0xba, 0xf8, 0x03][..], 0xba),
        (&[0x48, 0xc7, 0xc0, 0, 0, 0x10, 0][..], 0xc7),
        (&[0x48, 0x39, 0xc7][..], 0x39),
        (&[0x75, 0x24][..], 0x75),
        (&[0x49, 0x89, 0xf8][..], 0x89),
        (&[0x48, 0x8d, 0x3d, 0x09, 0x2b, 0, 0][..], 0x8d),
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

#[test]
fn cs_call_rel32_keeps_the_unprefixed_call_rel32_outcome_in_the_paged_probe_profile() {
    // S4-38L7af: `2E E8 cd` is `E8 cd`; this profile treats the two spellings the same way (both
    // are refused with opcode 0xe8 and without mutation).
    for (prefixed, plain) in [
        (
            &[0x2e, 0xe8, 0x46, 0x84, 0x7a, 0xfe][..],
            &[0xe8, 0x46, 0x84, 0x7a, 0xfe][..],
        ),
        (
            &[0x3e, 0xe8, 0x10, 0x00, 0x00, 0x00][..],
            &[0xe8, 0x10, 0x00, 0x00, 0x00][..],
        ),
    ] {
        let mut outcomes = Vec::new();
        for bytes in [prefixed, plain] {
            let mut memory = Memory::new(bytes);
            let mut cpu = state();
            cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
                PagedProbeExit::Unsupported { opcode: 0xe8 },
                "{bytes:02x?}"
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(cpu, before, "{bytes:02x?}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            outcomes.push(result.exit);
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
}

#[test]
fn fs_gs_movsxd_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ak belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x4c, 0x63, 0x3d, 0x25, 0xb0, 0x05, 0x03][..],
        &[0x64, 0x48, 0x63, 0x05, 0x34, 0x12, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0x63 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_movsx_byte_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7ba belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x48, 0x0f, 0xbe, 0x01][..],
        &[0x65, 0x4c, 0x0f, 0xbe, 0x42, 0x0c][..],
        &[0x65, 0x0f, 0xbe, 0x01][..],
        &[0x64, 0x48, 0x0f, 0xbe, 0x05, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.gpr[Register::Rcx as usize] = DATA;
        cpu.gpr[Register::Rdx as usize] = DATA;
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
            PagedProbeExit::Unsupported { opcode: 0xbe },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_movzx_byte_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bf belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x0f, 0xb6, 0x07][..],
        &[0x65, 0x48, 0x0f, 0xb6, 0x01][..],
        &[0x65, 0x4c, 0x0f, 0xb6, 0x42, 0x0c][..],
        &[0x64, 0x48, 0x0f, 0xb6, 0x05, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.gpr[Register::Rcx as usize] = DATA;
        cpu.gpr[Register::Rdx as usize] = DATA;
        cpu.gpr[Register::Rdi as usize] = DATA;
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
            PagedProbeExit::Unsupported { opcode: 0xb6 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_cmpxchg_byte_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bg belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x0f, 0xb0, 0x0f][..],
        &[0x64, 0x0f, 0xb0, 0x0f][..],
        &[0x65, 0x4c, 0x0f, 0xb0, 0x42, 0x0c][..],
        &[0x64, 0x0f, 0xb0, 0x0d, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.gpr[Register::Rcx as usize] = DATA;
        cpu.gpr[Register::Rdx as usize] = DATA;
        cpu.gpr[Register::Rdi as usize] = DATA;
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
            PagedProbeExit::Unsupported { opcode: 0xb0 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_cmpxchg_wide_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bo belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x48, 0x0f, 0xb1, 0x35, 0xcc, 0x16, 0xc1, 0x02][..],
        &[0x64, 0x0f, 0xb1, 0x0f][..],
        &[0x65, 0x48, 0x0f, 0xb1, 0x0f][..],
        &[0x64, 0x4c, 0x0f, 0xb1, 0x42, 0x0c][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.gpr[Register::Rcx as usize] = DATA;
        cpu.gpr[Register::Rdx as usize] = DATA;
        cpu.gpr[Register::Rdi as usize] = DATA;
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
            PagedProbeExit::Unsupported { opcode: 0xb1 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn fs_gs_mov_byte_store_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L7bb belongs to the CPL0 set only; this profile refuses the forms without mutation.
    for bytes in [
        &[0x65, 0x88, 0x01][..],
        &[0x64, 0x88, 0x01][..],
        &[0x65, 0x40, 0x88, 0x31][..],
        &[0x64, 0x88, 0x05, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rbx as usize] = DATA;
        cpu.gpr[Register::Rcx as usize] = DATA;
        cpu.gpr[Register::Rdx as usize] = DATA;
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
            PagedProbeExit::Unsupported { opcode: 0x88 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_star_unsupported_and_pure() {
    // S4-38L7ao: the IA32_STAR write belongs to the CPL0 set only. Every paged probe profile
    // keeps `WRMSR` (ECX = 0xC000_0081, EDX:EAX = 0x0023_0010:0) refused with the opcode 0x30.
    fn star_state(cr3: Option<u64>) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = 0xc000_0081;
        cpu.gpr[Register::Rax as usize] = 0;
        cpu.gpr[Register::Rdx as usize] = 0x0023_0010;
        cpu
    }
    let wrmsr = [0x0f, 0x30];

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = star_state(None);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = star_state(None);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = star_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = star_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_lstar_unsupported_and_pure() {
    // S4-38L7ap: the IA32_LSTAR write belongs to the CPL0 set only. Every paged probe profile
    // keeps `WRMSR` (ECX = 0xC000_0082, EDX:EAX = 0xffff_ffff:0x8100_0080) refused with 0x30.
    fn lstar_state(cr3: Option<u64>) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = 0xc000_0082;
        cpu.gpr[Register::Rax as usize] = 0x8100_0080;
        cpu.gpr[Register::Rdx as usize] = 0xffff_ffff;
        cpu
    }
    let wrmsr = [0x0f, 0x30];

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = lstar_state(None);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = lstar_state(None);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = lstar_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = lstar_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_cstar_unsupported_and_pure() {
    // S4-38L7aq: the IA32_CSTAR write belongs to the CPL0 set only. Every paged probe profile
    // keeps `WRMSR` (ECX = 0xC000_0083, EDX:EAX = 0xffff_ffff:0x8100_1ab0) refused with 0x30.
    fn cstar_state(cr3: Option<u64>) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = 0xc000_0083;
        cpu.gpr[Register::Rax as usize] = 0x8100_1ab0;
        cpu.gpr[Register::Rdx as usize] = 0xffff_ffff;
        cpu
    }
    let wrmsr = [0x0f, 0x30];

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = cstar_state(None);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = cstar_state(None);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = cstar_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = cstar_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_sysenter_cs_unsupported_and_pure() {
    // S4-38L7ar: the IA32_SYSENTER_CS write belongs to the CPL0 set only. Every paged probe
    // profile keeps `WRMSR` (ECX = 0x174, EDX:EAX = 0:0x10) refused with 0x30. The paged probe
    // profiles take no MSR profile, so the `SysenterProbeV1` precedence of the set does not apply.
    fn sysenter_state(cr3: Option<u64>) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = 0x174;
        cpu.gpr[Register::Rax as usize] = 0x10;
        cpu.gpr[Register::Rdx as usize] = 0;
        cpu
    }
    let wrmsr = [0x0f, 0x30];

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = sysenter_state(None);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = sysenter_state(None);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = sysenter_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = sysenter_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_sysenter_esp_and_eip_unsupported_and_pure() {
    // S4-38L7as: the IA32_SYSENTER_ESP/EIP writes belong to the CPL0 set only. Every paged probe
    // profile keeps `WRMSR` (ECX = 0x175 or 0x176, EDX:EAX = 0xfffffe00:0x3000) refused with 0x30. The paged probe
    // profiles take no MSR profile, so the `SysenterProbeV1` precedence of the set does not apply.
    fn sysenter_state(cr3: Option<u64>, index: u64) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = index;
        cpu.gpr[Register::Rax as usize] = 0x3000;
        cpu.gpr[Register::Rdx as usize] = 0xffff_fe00;
        cpu
    }
    let wrmsr = [0x0f, 0x30];
    for index in [0x175u64, 0x176] {
        let mut memory = Memory::new(&wrmsr);
        let mut cpu = sysenter_state(None, index);
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);

        let mut memory = Memory::new(&wrmsr);
        let mut cpu = sysenter_state(None, index);
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
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
        assert_eq!(cpu, before);

        let mut memory = Memory::new(&wrmsr);
        let mut cpu = sysenter_state(Some(0x1000), index);
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
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
        assert_eq!(cpu, before);

        let mut memory = Memory::new(&wrmsr);
        let mut cpu = sysenter_state(Some(0x1000), index);
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
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
        assert_eq!(cpu, before);
    }
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_syscall_mask_unsupported_and_pure() {
    // S4-38L7at: the IA32_SYSCALL_MASK write belongs to the CPL0 set only. Every paged probe
    // profile keeps `WRMSR` (ECX = 0xc0000084, EDX:EAX = 0:0x257fd5) refused with 0x30. The paged
    // probe profiles take no MSR profile.
    fn mask_state(cr3: Option<u64>) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = 0xc000_0084;
        cpu.gpr[Register::Rax as usize] = 0x25_7fd5;
        cpu.gpr[Register::Rdx as usize] = 0;
        cpu
    }
    let wrmsr = [0x0f, 0x30];

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = mask_state(None);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = mask_state(None);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = mask_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = mask_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);
}

#[test]
fn paged_probe_profiles_keep_wrmsr_ia32_kernel_gs_base_unsupported_and_pure() {
    // S4-38L7au: the IA32_KERNEL_GS_BASE write belongs to the CPL0 set only. Every paged probe
    // profile keeps `WRMSR` (ECX = 0xc0000102, EDX:EAX = 0xffff8880:0x12345678) refused with
    // 0x30. The paged probe profiles take no MSR profile.
    fn kgs_state(cr3: Option<u64>) -> LongModeState {
        let mut cpu = state();
        cpu.cr3 = cr3;
        cpu.gpr[Register::Rcx as usize] = 0xc000_0102;
        cpu.gpr[Register::Rax as usize] = 0x1234_5678;
        cpu.gpr[Register::Rdx as usize] = 0xffff_8880;
        cpu
    }
    let wrmsr = [0x0f, 0x30];

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = kgs_state(None);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, rdtsc_ring0());
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = kgs_state(None);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = kgs_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);

    let mut memory = Memory::new(&wrmsr);
    let mut cpu = kgs_state(Some(0x1000));
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x30 });
    assert_eq!(cpu, before);
}

#[test]
fn out_in_imm8_stay_unsupported_in_the_paged_probe_profiles() {
    // S4-38L7bk belongs to the CPL0 set only (member `pio_u8`); the paged probe profiles refuse
    // `E6 ib` / `E4 ib` with their own opcode, without a port access and without mutation.
    for (bytes, opcode) in [
        (&[0xe6, 0x43][..], 0xe6u8),
        (&[0xe4, 0x40][..], 0xe4),
        (&[0xe6, 0x00][..], 0xe6),
        (&[0xe4, 0xff][..], 0xe4),
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
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());

        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let result = execute::step_paged_stack_probe(
            PagedCpl0StackProbeV1,
            &mut cpu,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
            DEFINED,
        );
        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(cpu, before);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn int3_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9e belongs to the CPL0 set only; this profile refuses `CC` without mutation.
    let mut memory = Memory::new(&[0xcc, 0x90]);
    let mut cpu = state();
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(
        &mut cpu,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xcc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn ds_xadd_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9g belongs to the CPL0 set only; this profile refuses XADD with a DS override (dword
    // and qword) without mutation.
    for bytes in [
        &[0x3e, 0x48, 0x0f, 0xc1, 0x43, 0x58][..],
        &[0x3e, 0x0f, 0xc1, 0x43, 0x58][..],
        &[0x3e, 0x44, 0x0f, 0xc1, 0x05, 0x34, 0x12, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0xc1 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn ds_cmpxchg_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9h belongs to the CPL0 set only; this profile refuses CMPXCHG with a DS override
    // (dword and qword) without mutation.
    for bytes in [
        &[0x3e, 0x48, 0x0f, 0xb1, 0x43, 0x58][..],
        &[0x3e, 0x0f, 0xb1, 0x43, 0x58][..],
        &[0x3e, 0x44, 0x0f, 0xb1, 0x05, 0x34, 0x12, 0x00, 0x00][..],
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
            PagedProbeExit::Unsupported { opcode: 0xb1 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn segment_logic_load_stays_unsupported_in_the_paged_probe_profile() {
    // S4-38L9i belongs to the CPL0 set only; this profile refuses OR/AND/XOR r, m with FS/GS
    // (dword and qword) without mutation, with the exact opcode.
    for (bytes, opcode) in [
        (&[0x65, 0x48, 0x0b, 0x05, 0xb4, 0x7c, 0xce, 0x01][..], 0x0b),
        (&[0x64, 0x0b, 0x15, 0x34, 0x12, 0x00, 0x00][..], 0x0b),
        (&[0x65, 0x48, 0x23, 0x15, 0x34, 0x12, 0x00, 0x00][..], 0x23),
        (&[0x64, 0x33, 0x15, 0x34, 0x12, 0x00, 0x00][..], 0x33),
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
fn plain_dword_xadd_and_cmpxchg_stay_unsupported_in_the_paged_probe_profile() {
    // S4-38L9k belongs to the CPL0 set only (the bare memory forms run there as the LOCK forms);
    // this profile refuses the bare dword memory forms of XADD and CMPXCHG without mutation.
    for (bytes, opcode) in [
        (&[0x0f, 0xc1, 0x43, 0x58][..], 0xc1u8),
        (&[0x44, 0x0f, 0xc1, 0x05, 0x34, 0x12, 0x00, 0x00][..], 0xc1),
        (&[0x0f, 0xb1, 0x43, 0x58][..], 0xb1),
        (&[0x44, 0x0f, 0xb1, 0x05, 0x34, 0x12, 0x00, 0x00][..], 0xb1),
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
            PagedProbeExit::Unsupported {
                opcode: opcode.into()
            },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}
