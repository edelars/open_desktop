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
    read_addresses: Vec<(u64, usize)>,
    writes: usize,
    write_addresses: Vec<u64>,
    fail_fetch: Option<MemoryError>,
    fail_fetch_at: Option<(u64, MemoryError)>,
    fail_read: Option<MemoryError>,
    fail_read_at: Option<(u64, MemoryError)>,
    fail_write: Option<MemoryError>,
    fail_write_on: Option<(usize, MemoryError)>,
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
            read_addresses: Vec::new(),
            writes: 0,
            write_addresses: Vec::new(),
            fail_fetch: None,
            fail_fetch_at: None,
            fail_read: None,
            fail_read_at: None,
            fail_write: None,
            fail_write_on: None,
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
        self.read_addresses.push((address, destination.len()));
        if let Some(error) = &self.fail_read {
            return Err(error.clone());
        }
        if let Some((fault_address, error)) = &self.fail_read_at {
            let end = address.saturating_add(destination.len() as u64);
            if address <= *fault_address && *fault_address < end {
                return Err(error.clone());
            }
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
        self.write_addresses.push(address);
        if let Some((write_number, error)) = &self.fail_write_on
            && self.writes == *write_number
        {
            return Err(error.clone());
        }
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

fn memory_with_code_at(code: &[u8], rip: u64) -> Memory {
    let mut memory = Memory::new(&[]);
    let page = rip & !0xfff;
    let offset = (rip & 0xfff) as usize;
    memory.pages.insert(page, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + offset..CODE_PHYSICAL + offset + code.len()].copy_from_slice(code);
    memory
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
fn firmware_entry_mov_esi_handoff_gpa_zero_extends_and_preserves_state() {
    let bytes = [0xbe, 0x00, 0x00, 0x10, 0x00];
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
    expected.gpr[Register::Rsi as usize] = 0x0010_0000;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_mov_esi_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0xbe, 0x01, 0x00, 0x10, 0x00], 0xbe), // different imm32
        (&[0xbf, 0x00, 0x00, 0x10, 0x00], 0xbf), // EDI destination
        (&[0xbd, 0x00, 0x00, 0x10, 0x00], 0xbd), // EBP destination
        (&[0x48, 0xbe, 0, 0, 0x10, 0, 0, 0, 0, 0], 0xbe), // MOV RSI,imm64
        (&[0x40, 0xbe, 0, 0, 0x10, 0], 0xbe),    // redundant REX
        (&[0x41, 0xbe, 0, 0, 0x10, 0], 0xbe),    // R14D destination
        (&[0xf3, 0xbe, 0, 0, 0x10, 0], 0xbe),    // REP prefix
        (&[0x67, 0xbe, 0, 0, 0x10, 0], 0xbe),    // address-size prefix
        (&[0x2e, 0xbe, 0, 0, 0x10, 0], 0xbe),    // segment prefix
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
fn firmware_entry_mov_esi_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffc;
    memory.bytes[CODE_PHYSICAL + 0xffc..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0xbe, 0x00, 0x00, 0x10]);
    memory.fail_fetch_at = Some((cpu.rip + 4, page_fault(0x10, cpu.rip + 4)));
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
fn firmware_entry_mov_esi_is_unsupported_in_cpl3_profile() {
    let bytes = [0xbe, 0x00, 0x00, 0x10, 0x00];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xbe });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_mov_edx_block_size_zero_extends_and_preserves_state() {
    let bytes = [0xba, 0x00, 0x00, 0x01, 0x00];
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
    expected.gpr[Register::Rdx as usize] = 0x0001_0000;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_mov_edx_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0xba, 0x01, 0x00, 0x01, 0x00], 0xba), // different imm32
        (&[0xb9, 0x00, 0x00, 0x01, 0x00], 0xb9), // ECX destination
        (&[0xbb, 0x00, 0x00, 0x01, 0x00], 0xbb), // EBX destination
        (&[0x48, 0xba, 0, 0, 1, 0, 0, 0, 0, 0], 0xba), // MOV RDX,imm64
        (&[0x40, 0xba, 0, 0, 1, 0], 0xba),       // redundant REX
        (&[0x41, 0xba, 0, 0, 1, 0], 0xba),       // R10D destination
        (&[0xf3, 0xba, 0, 0, 1, 0], 0xba),       // REP prefix
        (&[0x67, 0xba, 0, 0, 1, 0], 0xba),       // address-size prefix
        (&[0x2e, 0xba, 0, 0, 1, 0], 0xba),       // segment prefix
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
fn firmware_entry_mov_edx_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffc;
    memory.bytes[CODE_PHYSICAL + 0xffc..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0xba, 0x00, 0x00, 0x01]);
    memory.fail_fetch_at = Some((cpu.rip + 4, page_fault(0x10, cpu.rip + 4)));
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
fn firmware_entry_mov_edx_is_unsupported_in_cpl3_profile() {
    let bytes = [0xba, 0x00, 0x00, 0x01, 0x00];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xba });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_helper_mov_rdi_handoff_magic_sets_only_rdi() {
    let bytes = [0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46];
    let magic: u64 = 0x464f_444e_4148_444f;
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::Rdi as usize] = magic;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_helper_mov_rdi_handoff_magic_neighbours_stay_unsupported() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0xbf, 0, 0, 0, 0, 0, 0, 0, 0], 0xbf), // zero immediate
        (
            &[0x48, 0xbf, 0x4e, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46],
            0xbf,
        ), // low bit
        (
            &[0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x47],
            0xbf,
        ), // high bit
        (
            &[0x48, 0xbe, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46],
            0xbe,
        ), // RSI
        (
            &[0x48, 0xb8, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46],
            0xb8,
        ), // RAX
        (
            &[0x49, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46],
            0xbf,
        ), // R15
        (
            &[0x40, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46],
            0xbf,
        ), // 32-bit RDI
        (
            &[
                0x66, 0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46,
            ],
            0xbf,
        ), // operand prefix
        (
            &[
                0xf3, 0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46,
            ],
            0xbf,
        ), // REP prefix
        (&[0x48, 0xc7, 0xc7, 0x4f, 0x44, 0x48, 0x41], 0xc7), // C7 immediate form
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
fn firmware_helper_mov_rdi_handoff_magic_fetch_fault_does_not_commit() {
    let bytes = [0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46];
    let mut memory = Memory::new(&[]);
    let mut cpu = state();
    cpu.rip = RIP + 0xff7;
    memory.bytes[CODE_PHYSICAL + 0xff7..CODE_PHYSICAL + 0x1000].copy_from_slice(&bytes[..9]);
    memory.fail_fetch_at = Some((cpu.rip + 9, page_fault(0x10, cpu.rip + 9)));
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
fn firmware_helper_mov_rdi_handoff_magic_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xbf });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_r8_imm64_sets_only_r8() {
    let bytes = [0x49, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
    let value = 0x7fff_ffff_ffff_fffc;
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::R8 as usize] = value;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_r8_imm64_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (
            &[0x49, 0xb8, 0xfb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
            0xb8,
        ), // adjacent immediate
        (
            &[0x48, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
            0xb8,
        ), // RAX
        (
            &[0x49, 0xb9, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
            0xb9,
        ), // R9
        (&[0x41, 0xb8, 0xfc, 0xff, 0xff, 0xff], 0xb8), // dword R8
        (
            &[0x4d, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
            0xb8,
        ), // other REX
        (
            &[
                0x66, 0x49, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
            ],
            0xb8,
        ), // operand-size prefix
        (
            &[
                0xf3, 0x49, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
            ],
            0xb8,
        ), // REP prefix
        (&[0x49, 0xc7, 0xc0, 0xfc, 0xff, 0xff, 0xff], 0xc7), // sign-extended imm32 form
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
fn rust_helper_mov_r8_imm64_fetch_fault_is_atomic() {
    let bytes = [0x49, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
    let rip = RIP + 0xff7;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xff7..CODE_PHYSICAL + 0x1000].copy_from_slice(&bytes[..9]);
    memory.fail_fetch_at = Some((rip + 9, page_fault(0x10, rip + 9)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 9),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_r8_imm64_is_unsupported_in_cpl3_profile() {
    let bytes = [0x49, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
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
fn rust_helper_mov_r9d_imm32_zero_extends_and_preserves_state() {
    let bytes = [0x41, 0xb9, 0xff, 0xff, 0xff, 0xff]; // MOV R9D,0xffffffff
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::R9 as usize] = 0xffff_ffff;
    expected.rip += bytes.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_r9d_imm32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0xb9, 0xfe, 0xff, 0xff, 0xff], 0xb9), // adjacent immediate
        (&[0x41, 0xb8, 0xff, 0xff, 0xff, 0xff], 0xb8), // R8D
        (&[0x41, 0xba, 0xff, 0xff, 0xff, 0xff], 0xba), // R10D
        (&[0x40, 0xb9, 0xff, 0xff, 0xff, 0xff], 0xb9), // ECX without REX.B
        (&[0x49, 0xb9, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0], 0xb9), // qword R9
        (&[0x66, 0x41, 0xb9, 0xff, 0xff], 0xb9),       // word R9
        (&[0xf3, 0x41, 0xb9, 0xff, 0xff, 0xff, 0xff], 0xb9), // REP prefix
        (&[0x41, 0xc7, 0xc1, 0xff, 0xff, 0xff, 0xff], 0xc7), // alternate C7 imm32 form
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
fn rust_helper_mov_r9d_imm32_final_immediate_fetch_fault_is_atomic() {
    let bytes = [0x41, 0xb9, 0xff, 0xff, 0xff, 0xff];
    let rip = RIP + 0xffb;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffb..CODE_PHYSICAL + 0x1000].copy_from_slice(&bytes[..5]);
    memory.fail_fetch_at = Some((rip + 5, page_fault(0x10, rip + 5)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 5),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_r9d_imm32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0xb9, 0xff, 0xff, 0xff, 0xff];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
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
fn rust_helper_cmp_rdx_rbp_uses_qword_operands_and_preserves_registers() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const ARITH_FLAGS: u64 = CF | PF | AF | ZF | SF | OF;
    let code = [0x48, 0x39, 0xea]; // CMP RDX,RBP
    let cases = [
        (0x1234_5678_9abc_def0, 0x1234_5678_9abc_def0, PF | ZF),
        (0, 1, CF | PF | AF | SF),
        (1, 0, 0),
        (i64::MIN as u64, 1, PF | AF | OF),
    ];

    for (rdx, rbp, cmp_flags) in cases {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rdx as usize] = rdx;
        cpu.gpr[Register::Rbp as usize] = rbp;
        cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | ARITH_FLAGS;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !(1 << 10);

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired, "{rdx:#x} - {rbp:#x}");
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | ARITH_FLAGS);
        let mut expected = before;
        expected.rflags = (expected.rflags & !ARITH_FLAGS) | cmp_flags;
        expected.rip += code.len() as u64;
        assert_eq!(cpu, expected, "{rdx:#x} - {rbp:#x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_rdx_rbp_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let code = [0x48, 0x39];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_cmp_rdx_rbp_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x39, 0xeb], 0x39),       // CMP RBX,RBP
        (&[0x48, 0x39, 0xd5], 0x39),       // CMP RBP,RDX
        (&[0x48, 0x3b, 0xea], 0x3b),       // reverse opcode direction
        (&[0x39, 0xea], 0x39),             // dword CMP EDX,EBP
        (&[0x66, 0x48, 0x39, 0xea], 0x39), // extra operand-size prefix
        (&[0x40, 0x48, 0x39, 0xea], 0x39), // redundant REX
        (&[0x48, 0x39, 0x2a], 0x39),       // memory destination
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_rdx_rbp_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x39, 0xea];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
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
fn firmware_helper_cmp_handoff_magic_reads_qword_and_updates_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const ARITH_FLAGS: u64 = CF | PF | AF | ZF | SF | OF;
    let code = [0x48, 0x39, 0x3e]; // CMP qword ptr [RSI],RDI
    let magic: u64 = 0x464f_444e_4148_444f;
    let mut memory = Memory::new(&code);
    memory.bytes[DATA_PHYSICAL..DATA_PHYSICAL + 8].copy_from_slice(&magic.to_le_bytes());
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::Rdi as usize] = magic;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(CF | AF | ZF);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined | ARITH_FLAGS);
    let mut expected = before;
    expected.rflags = (expected.rflags & !ARITH_FLAGS) | PF | ZF;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA, 8)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_helper_cmp_handoff_magic_read_fault_preserves_state() {
    let code = [0x48, 0x39, 0x3e];
    let mut memory = Memory::new(&code);
    memory.fail_read = Some(page_fault(0x0, DATA));
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x0),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA, 8)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_helper_cmp_handoff_magic_noncanonical_pointer_faults_before_read() {
    let code = [0x48, 0x39, 0x3e];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = 0x0000_8000_0000_0000;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    match result.exit {
        PagedProbeExit::Fault(fault) => assert_eq!(fault.vector, 13),
        other => panic!("expected #GP for noncanonical RSI, got {other:?}"),
    }
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.read_addresses.is_empty());
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_helper_cmp_handoff_magic_injected_guest_memory_read_failure_is_atomic() {
    let code = [0x48, 0x39, 0x3e];
    let mut memory = Memory::new(&code);
    let injected_fault_address = DATA + 4;
    memory.fail_read_at = Some((
        injected_fault_address,
        page_fault(0x0, injected_fault_address),
    ));
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x0),
            linear_address: Some(injected_fault_address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA, 8)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_helper_cmp_handoff_magic_neighbours_stay_unsupported() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x39, 0x3f], 0x39),       // [RDI],RDI
        (&[0x48, 0x39, 0x06], 0x39),       // [RSI],RAX
        (&[0x48, 0x39, 0x7e, 0x00], 0x39), // [RSI+0],RDI
        (&[0x48, 0x39, 0x3c, 0x24], 0x39), // [RSP],RDI
        (&[0x4c, 0x39, 0x3e], 0x39),       // [RSI],R14
        (&[0x39, 0x3e], 0x39),             // dword comparison
        (&[0x66, 0x48, 0x39, 0x3e], 0x39), // operand-size prefix
        (&[0x67, 0x48, 0x39, 0x3e], 0x39), // address-size prefix
        (&[0xf3, 0x48, 0x39, 0x3e], 0x39), // REP prefix
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
        assert_eq!(memory.reads, 0);
        assert_eq!(memory.writes, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_helper_cmp_handoff_magic_is_unsupported_in_cpl3_profile() {
    let code = [0x48, 0x39, 0x3e];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsi as usize] = DATA;
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
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.writes, 0);
}

#[test]
fn firmware_entry_sub_rcx_rdi_updates_result_flags_and_state() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const ARITH_FLAGS: u64 = CF | PF | AF | ZF | SF | OF;
    let code = [0x48, 0x29, 0xf9]; // Intel: SUB RCX,RDI; AT&T: subq %rdi,%rcx
    let cases = [
        (0x1234, 0x1234, 0x0, PF | ZF),
        (0, 1, u64::MAX, CF | PF | AF | SF),
        (i64::MIN as u64, 1, i64::MAX as u64, PF | AF | OF),
    ];

    for (rcx, rdi, expected_result, expected_flags) in cases {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rcx as usize] = rcx;
        cpu.gpr[Register::Rdi as usize] = rdi;
        cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | ARITH_FLAGS;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !(1 << 10);

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired, "{rcx:#x} - {rdi:#x}");
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | ARITH_FLAGS);
        let mut expected = before;
        expected.gpr[Register::Rcx as usize] = expected_result;
        expected.rflags = (expected.rflags & !ARITH_FLAGS) | expected_flags;
        expected.rip += code.len() as u64;
        assert_eq!(cpu, expected, "{rcx:#x} - {rdi:#x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_sub_fetch_fault_on_modrm_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x48, 0x29]);
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
fn firmware_entry_sub_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x29, 0xf8], 0x29),       // SUB RAX,RDI
        (&[0x48, 0x29, 0xf1], 0x29),       // SUB RCX,RSI
        (&[0x48, 0x2b, 0xf9], 0x2b),       // reverse direction: SUB RDI,RCX
        (&[0x29, 0xf9], 0x29),             // dword SUB ECX,EDI
        (&[0x66, 0x48, 0x29, 0xf9], 0x29), // extra operand-size prefix
        (&[0x40, 0x48, 0x29, 0xf9], 0x29), // redundant REX prefix
        (&[0x48, 0x29, 0x39], 0x29),       // memory destination
        (&[0x48, 0x01, 0xf9], 0x01),       // ADD RCX,RDI
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
fn firmware_entry_sub_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x29, 0xf9];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x29 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_xor_eax_eax_zero_extends_and_updates_logical_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const LOGIC_DEFINED: u64 = CF | PF | ZF | SF | OF;
    let code = [0x31, 0xc0]; // XOR EAX,EAX
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rax as usize] = 0xffff_ffff_a5a5_5a5a;
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | CF | AF | SF | OF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !ZF;

    let initial_defined = defined | AF;
    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        result.rflags_defined,
        (initial_defined | LOGIC_DEFINED) & !AF
    );
    let mut expected = before;
    expected.gpr[Register::Rax as usize] = 0; // EAX write zero-extends into RAX.
    expected.rflags = (expected.rflags & !(CF | PF | ZF | SF | OF)) | PF | ZF;
    // XOR leaves AF's concrete bit untouched although its definedness is cleared.
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_xor_ecx_ecx_zero_extends_and_updates_logical_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const LOGIC_DEFINED: u64 = CF | PF | ZF | SF | OF;
    let code = [0x31, 0xc9]; // XOR ECX,ECX
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rcx as usize] = 0xffff_ffff_a5a5_5a5a;
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | CF | AF | SF | OF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !ZF;
    let initial_defined = defined | AF;

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        result.rflags_defined,
        (initial_defined | LOGIC_DEFINED) & !AF
    );
    let mut expected = before;
    expected.gpr[Register::Rcx as usize] = 0; // ECX write zero-extends into RCX.
    expected.rflags = (expected.rflags & !(CF | PF | ZF | SF | OF)) | PF | ZF;
    // XOR leaves AF's concrete bit untouched although its definedness is cleared.
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r10d_r10d_zero_extends_and_updates_logical_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const LOGIC_DEFINED: u64 = CF | PF | ZF | SF | OF;
    let code = [0x45, 0x31, 0xd2]; // XOR R10D,R10D
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R10 as usize] = 0xffff_ffff_a5a5_5a5a;
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | CF | AF | SF | OF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = (DEFINED & !ZF) | AF;

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        result.rflags_defined,
        (initial_defined | LOGIC_DEFINED) & !AF
    );
    let mut expected = before;
    expected.gpr[Register::R10 as usize] = 0; // R10D write zero-extends into R10.
    expected.rflags = (expected.rflags & !(CF | PF | ZF | SF | OF)) | PF | ZF;
    // XOR leaves AF's concrete bit untouched although its definedness is cleared.
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r11d_r11d_zero_extends_and_updates_logical_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const LOGIC_DEFINED: u64 = CF | PF | ZF | SF | OF;
    let code = [0x45, 0x31, 0xdb]; // XOR R11D,R11D
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R11 as usize] = 0xffff_ffff_a5a5_5a5a;
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | CF | AF | SF | OF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = (DEFINED & !ZF) | AF;

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        result.rflags_defined,
        (initial_defined | LOGIC_DEFINED) & !AF
    );
    let mut expected = before;
    expected.gpr[Register::R11 as usize] = 0; // R11D write zero-extends into R11.
    expected.rflags = (expected.rflags & !(CF | PF | ZF | SF | OF)) | PF | ZF;
    // XOR leaves AF's concrete bit untouched although its definedness is cleared.
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r9d_r14d_zero_extends_and_updates_logical_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const LOGIC_DEFINED: u64 = CF | PF | ZF | SF | OF;
    let code = [0x45, 0x31, 0xf1]; // XOR R9D,R14D
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0xdead_beef_ffff_ffff;
    cpu.gpr[Register::R14 as usize] = 0x1122_3344_0000_0001;
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | CF | AF | SF | OF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = (DEFINED & !ZF) | AF;

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        result.rflags_defined,
        (initial_defined | LOGIC_DEFINED) & !AF
    );
    let mut expected = before;
    expected.gpr[Register::R9 as usize] = 0xffff_fffe; // R9D write zero-extends into R9.
    expected.rflags = (expected.rflags & !(CF | PF | ZF | SF | OF)) | SF;
    // XOR leaves AF's concrete bit untouched even though AF becomes undefined.
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r12d_sib_dword_read_uses_exact_address_and_flags() {
    const CF: u64 = 1 << 0;
    const PF: u64 = 1 << 2;
    const AF: u64 = 1 << 4;
    const ZF: u64 = 1 << 6;
    const SF: u64 = 1 << 7;
    const OF: u64 = 1 << 11;
    const LOGIC_DEFINED: u64 = CF | PF | ZF | SF | OF;
    const EFFECTIVE: u64 = 0x202_728; // R9=0x2a, scaled by four, plus 0x202680.
    let code = [0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0x00];
    let mut memory = Memory::new(&code);
    memory.pages.insert(0x202_000, DATA_PHYSICAL);
    memory.bytes[DATA_PHYSICAL + 0x728..DATA_PHYSICAL + 0x72c]
        .copy_from_slice(&0xa5a5_5a5au32.to_le_bytes());
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0x2a;
    cpu.gpr[Register::R12 as usize] = 0xdead_beef_0000_0001;
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | CF | AF | OF;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = (DEFINED & !ZF) | AF;

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        result.rflags_defined,
        (initial_defined | LOGIC_DEFINED) & !AF
    );
    let mut expected = before;
    expected.gpr[Register::R12 as usize] = 0xa5a5_5a5b; // Dword write zero-extends.
    expected.rflags = (expected.rflags & !(CF | PF | ZF | SF | OF)) | SF;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.read_addresses, [(EFFECTIVE, 4)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r12d_sib_dword_read_fault_does_not_commit() {
    const EFFECTIVE: u64 = 0x202_728;
    let code = [0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0x00];
    let mut memory = Memory::new(&code);
    memory.pages.insert(0x202_000, DATA_PHYSICAL);
    memory.fail_read_at = Some((EFFECTIVE, page_fault(0, EFFECTIVE)));
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0x2a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(EFFECTIVE),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.read_addresses, [(EFFECTIVE, 4)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r12d_sib_noncanonical_effective_address_does_not_read_or_commit() {
    let code = [0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0x00];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0x0000_2000_0000_0000;
    let before = cpu.clone();
    let mut pio = Pio::default();

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
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r12d_sib_fetch_fault_on_final_displacement_byte_does_not_commit() {
    let mut memory = Memory::new(&[]);
    let code = [0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20];
    memory.bytes[CODE_PHYSICAL + 0xff9..CODE_PHYSICAL + 0xff9 + code.len()].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = RIP + 0xff9;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 7, page_fault(0x10, cpu.rip + 7)));

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
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_xor_r12d_sib_dword_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x46, 0x31, 0x24, 0x8d, 0x80, 0x26, 0x20, 0], 0x31), // Reverse direction.
        (&[0x46, 0x33, 0x2c, 0x8d, 0x80, 0x26, 0x20, 0], 0x33), // R13D destination.
        (&[0x46, 0x33, 0x24, 0x4d, 0x80, 0x26, 0x20, 0], 0x33), // Scale 2.
        (&[0x46, 0x33, 0x24, 0x8c, 0x80, 0x26, 0x20, 0], 0x33), // RSP base.
        (&[0x46, 0x33, 0x24, 0x8d, 0x81, 0x26, 0x20, 0], 0x33), // Other displacement.
        (&[0x4e, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0], 0x33), // REX.W.
        (&[0x66, 0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0], 0x33), // Word.
        (&[0xf3, 0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0], 0x33), // Prefix.
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
fn rust_helper_xor_r12d_sib_dword_is_unsupported_in_cpl3_profile() {
    let bytes = [0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0x00];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x46 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_xor_r9d_r14d_fetch_fault_on_modrm_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe] = 0x45;
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x31;
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
fn rust_helper_xor_r9d_r14d_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x45, 0x31, 0xf0], 0x31),       // XOR R8D,R14D
        (&[0x45, 0x31, 0xf9], 0x31),       // XOR R9D,R15D
        (&[0x45, 0x33, 0xf1], 0x33),       // reverse-direction opcode
        (&[0x4d, 0x31, 0xf1], 0x31),       // qword width
        (&[0x66, 0x45, 0x31, 0xf1], 0x31), // word width
        (&[0xf3, 0x45, 0x31, 0xf1], 0x31), // prefix
        (&[0x45, 0x31, 0x71, 0x01], 0x31), // memory destination
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
fn firmware_entry_xor_fetch_fault_on_modrm_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x31;
    let mut cpu = state();
    cpu.rip = RIP + 0xfff;
    cpu.gpr = gpr_patterns();
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
fn rust_helper_xor_r10d_fetch_fault_on_modrm_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe] = 0x45;
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x31;
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
fn rust_helper_xor_r11d_fetch_fault_on_modrm_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe] = 0x45;
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x31;
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
fn firmware_entry_xor_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x31, 0xd2], 0x31),       // XOR EDX,EDX
        (&[0x33, 0xc0], 0x33),       // reverse direction opcode
        (&[0x48, 0x31, 0xc0], 0x31), // qword XOR
        (&[0x66, 0x31, 0xc0], 0x31), // word XOR
        (&[0x40, 0x31, 0xc0], 0x31), // REX-prefixed XOR
        (&[0x41, 0x31, 0xc0], 0x31), // XOR R8D,EAX
        (&[0x45, 0x31, 0xda], 0x31), // XOR R10D,R11D
        (&[0x45, 0x31, 0xd3], 0x31), // XOR R11D,R10D
        (&[0x45, 0x33, 0xd2], 0x33), // reverse direction opcode
        (&[0x31, 0x00], 0x31),       // memory destination
        (&[0x01, 0xc0], 0x01),       // ADD EAX,EAX
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
fn firmware_entry_xor_is_unsupported_in_cpl3_profile() {
    for bytes in [[0x45, 0x31, 0xd2], [0x45, 0x31, 0xdb], [0x45, 0x31, 0xf1]] {
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
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x45 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn rust_helper_operand_size_prefixed_nop_advances_rip_only() {
    let code = [0x66, 0x90]; // NOP (XCHG AX,AX encoding)
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | (1 << 6) | (1 << 11);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = DEFINED & !(1 << 6);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, initial_defined);
    let mut expected = before;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_operand_size_prefixed_nop_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x66;
    let mut cpu = state();
    cpu.rip = RIP + 0xfff;
    cpu.gpr = gpr_patterns();
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
fn rust_helper_prefixed_nop_neighbors_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x90], 0x90),             // bare NOP remains outside this slice
        (&[0x66, 0x91], 0x91),       // XCHG AX,CX
        (&[0xf3, 0x90], 0x90),       // PAUSE
        (&[0x40, 0x90], 0x90),       // REX-prefixed XCHG
        (&[0x2e, 0x66, 0x90], 0x90), // extra segment prefix
        (&[0x66, 0x40, 0x90], 0x90), // extra REX prefix
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
fn rust_helper_operand_size_prefixed_nop_stays_unsupported_in_cpl3_profile() {
    let bytes = [0x66, 0x90];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x66 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_rep_stosb_writes_al_and_updates_rdi_rcx_by_df() {
    let code = [0xf3, 0xaa]; // REP STOSB, address-size 64
    for (df, start, expected_rdi, expected_offsets) in [
        (false, DATA, DATA + 3, [0usize, 1, 2]),
        (true, DATA + 2, DATA.wrapping_sub(1), [2usize, 1, 0]),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rax as usize] = 0xfeed_beef_cafe_00a5;
        cpu.gpr[Register::Rcx as usize] = 3;
        cpu.gpr[Register::Rdi as usize] = start;
        if df {
            cpu.rflags |= 1 << 10;
        } else {
            cpu.rflags &= !(1 << 10);
        }
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step(&mut cpu, &mut memory, &mut pio, ring0());

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, DEFINED);
        let mut expected = before;
        expected.gpr[Register::Rcx as usize] = 0;
        expected.gpr[Register::Rdi as usize] = expected_rdi;
        expected.rip += code.len() as u64;
        assert_eq!(cpu, expected);
        for offset in expected_offsets {
            assert_eq!(memory.bytes[DATA_PHYSICAL + offset], 0xa5);
        }
        assert_eq!(memory.writes, 3);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_rep_stosb_completes_count_at_work_limit() {
    let code = [0xf3, 0xaa];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rax as usize] = 0x5a;
    cpu.gpr[Register::Rcx as usize] = execute::REP_ITERATION_WORK_LIMIT;
    cpu.gpr[Register::Rdi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::Rcx as usize] = 0;
    expected.gpr[Register::Rdi as usize] = DATA + execute::REP_ITERATION_WORK_LIMIT;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    let start = DATA_PHYSICAL;
    let end = start + execute::REP_ITERATION_WORK_LIMIT as usize;
    assert!(memory.bytes[start..end].iter().all(|byte| *byte == 0x5a));
    assert_eq!(memory.writes, execute::REP_ITERATION_WORK_LIMIT as usize);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_rep_stosb_zero_count_retires_without_memory_access() {
    let code = [0xf3, 0xaa];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rcx as usize] = 0;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_rep_stosb_write_fault_preserves_only_completed_iterations() {
    let code = [0xf3, 0xaa];
    let mut memory = Memory::new(&code);
    memory.fail_write_on = Some((2, page_fault(0x2, DATA + 1)));
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rax as usize] = 0x5a;
    cpu.gpr[Register::Rcx as usize] = 3;
    cpu.gpr[Register::Rdi as usize] = DATA;
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu.gpr[Register::Rdi as usize], DATA + 1);
    assert_eq!(cpu.gpr[Register::Rcx as usize], 2);
    assert_eq!(cpu.rip, RIP); // restart the REP instruction after fault delivery
    assert_eq!(memory.bytes[DATA_PHYSICAL], 0x5a);
    assert_eq!(memory.writes, 2);
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.write_addresses, [DATA, DATA + 1]);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());

    // Resume after the fault: the completed first byte is not replayed, and the
    // remaining two elements continue at the saved RDI/RCX pair.
    memory.fail_write_on = None;
    let resumed = step(&mut cpu, &mut memory, &mut pio, ring0());
    assert_eq!(resumed.exit, PagedProbeExit::Retired);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(resumed.rflags_defined, DEFINED);
    assert_eq!(cpu.gpr[Register::Rcx as usize], 0);
    assert_eq!(cpu.gpr[Register::Rdi as usize], DATA + 3);
    assert_eq!(cpu.rip, RIP + code.len() as u64);
    assert_eq!(memory.write_addresses, [DATA, DATA + 1, DATA + 1, DATA + 2]);
    assert_eq!(memory.bytes[DATA_PHYSICAL..DATA_PHYSICAL + 3], [0x5a; 3]);
    assert_eq!(memory.writes, 4);
}

#[test]
fn firmware_entry_rep_stosb_refuses_counts_over_work_limit_without_mutation() {
    let code = [0xf3, 0xaa];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rcx as usize] = execute::REP_ITERATION_WORK_LIMIT + 1;
    cpu.gpr[Register::Rdi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xaa });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_rep_stosb_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0xf3;
    let mut cpu = state();
    cpu.rip = RIP + 0xfff;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rcx as usize] = 1;
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
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_rep_stosb_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0xaa], 0xaa),             // unprefixed STOSB
        (&[0xf2, 0xaa], 0xaa),       // REPNE STOSB
        (&[0xf3, 0x66, 0xaa], 0xaa), // extra operand prefix
        (&[0xf3, 0x48, 0xaa], 0xaa), // REX prefix
        (&[0xf3, 0x67, 0xaa], 0xaa), // address-size override
        (&[0xf3, 0xab], 0xab),       // REP STOS wider than byte
        (&[0xf3, 0xf3, 0xaa], 0xaa), // duplicate REP prefix
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
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_rep_stosb_is_unsupported_in_cpl3_profile() {
    let bytes = [0xf3, 0xaa];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xf3 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn firmware_entry_jnz_taken_and_not_taken_preserve_flags_and_definedness() {
    let defined = DEFINED & !(1 << 6);
    for displacement in [0x24u8, 0x2a, 0x38, 0x77] {
        let code = [0x75, displacement];
        for (zero, expected_rip) in [(false, RIP + 2 + u64::from(displacement)), (true, RIP + 2)] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18) | if zero { 1 << 6 } else { 0 };
            let before = cpu.clone();
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(
                result.exit,
                PagedProbeExit::Retired,
                "disp={displacement:#x}"
            );
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = expected_rip;
            assert_eq!(cpu, expected, "disp={displacement:#x}, ZF={zero}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn firmware_entry_jnz_checks_canonicality_only_when_taken() {
    let start = 0x0000_7fff_ffff_fffe;
    for displacement in [0x24u8, 0x2a, 0x38, 0x77] {
        let code = [0x75, displacement];
        let mut memory = Memory::new(&[]);
        memory.pages.insert(start & !0xfff, CODE_PHYSICAL);
        memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);

        let mut cpu = state();
        cpu.rip = start;
        cpu.rflags |= 1 << 6; // ZF=1: fall-through is non-canonical, but not checked here.
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
}

#[test]
fn firmware_entry_jnz_fetch_fault_on_rel8_does_not_commit() {
    // Put the opcode at the last byte of a mapped code page so every admitted rel8 faults.
    for displacement in [0x24u8, 0x2a, 0x38, 0x77] {
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
            }),
            "disp={displacement:#x}"
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
}

#[test]
fn firmware_entry_jnz_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x75, 0x23], 0x75),                // different displacement
        (&[0x75, 0x29], 0x75),                // neighbor below +0x2a
        (&[0x75, 0x2b], 0x75),                // neighbor above +0x2a
        (&[0x75, 0x37], 0x75),                // neighbor below +0x38
        (&[0x75, 0x39], 0x75),                // neighbor above +0x38
        (&[0x75, 0x76], 0x75),                // neighboring displacement
        (&[0x75, 0x78], 0x75),                // neighboring displacement
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
    let cases: &[&[u8]] = &[
        &[0x75, 0x24],
        &[0x75, 0x2a],
        &[0x75, 0x38],
        &[0x75, 0x77],
        &[0x0f, 0x85, 0xd6, 0, 0, 0],
    ];
    for bytes in cases {
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

        let opcode = if bytes[0] == 0x0f { 0x0f } else { 0x75 };
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn rust_helper_jnz_rel32_obeys_zf_and_preserves_flags_and_definedness() {
    let code = [0x0f, 0x85, 0xd6, 0, 0, 0]; // JNZ +0xd6
    for zero in [false, true] {
        for zf_defined in [false, true] {
            let defined = if zf_defined {
                DEFINED
            } else {
                DEFINED & !(1 << 6)
            };
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.rflags = (state().rflags & !(1 << 6)) | if zero { 1 << 6 } else { 0 };
            let before = cpu.clone();
            let expected_rip = if zero { RIP + 6 } else { RIP + 6 + 0xd6 };
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = expected_rip;
            assert_eq!(cpu, expected, "ZF={zero}, defined={zf_defined}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_jnz_rel32_checks_canonicality_only_for_taken_target() {
    let code = [0x0f, 0x85, 0xd6, 0, 0, 0];
    let rip = 0x0000_7fff_ffff_fffa;
    let mut memory = Memory::new(&[]);
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffa..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.rflags |= 1 << 6; // Not taken; noncanonical fall-through is deferred.
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.rip = 0x0000_8000_0000_0000;
    assert_eq!(cpu, expected);

    let rip = 0x0000_7fff_ffff_ff40;
    let mut memory = Memory::new(&[]);
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xf40..CODE_PHYSICAL + 0xf46].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.rflags &= !(1 << 6); // Taken target is noncanonical.
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

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
}

#[test]
fn rust_helper_jnz_rel32_fetch_fault_on_last_displacement_byte_is_atomic() {
    let rip = RIP + 0xffb;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffb..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x0f, 0x85, 0xd6, 0, 0]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 5, page_fault(0x10, rip + 5)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 5),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_jnz_rel32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0x85, 0xd5, 0, 0, 0], 0x85), // adjacent displacement
        (&[0x0f, 0x85, 0xd7, 0, 0, 0], 0x85), // adjacent displacement
        (&[0x0f, 0x84, 0xd6, 0, 0, 0], 0x84), // JZ rel32
        (&[0x0f, 0x85, 0x2a, 0, 0, 0], 0x85), // other JNZ rel32 target
        (&[0x66, 0x0f, 0x85, 0xd6, 0, 0, 0], 0x85), // operand-size prefix
        (&[0x40, 0x0f, 0x85, 0xd6, 0, 0, 0], 0x85), // REX prefix
        (&[0x75, 0xd6], 0x75),                // short JNZ
        (&[0xe9, 0xd6, 0, 0, 0], 0xe9),       // JMP rel32
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
fn rust_helper_jz_rel32_obeys_zf_and_preserves_flags_and_definedness() {
    let code = [0x0f, 0x84, 0xb4, 0, 0, 0]; // JZ +0xb4
    for zero in [false, true] {
        for zf_defined in [false, true] {
            let defined = if zf_defined {
                DEFINED
            } else {
                DEFINED & !(1 << 6)
            };
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.rflags = (state().rflags & !(1 << 6)) | if zero { 1 << 6 } else { 0 };
            let before = cpu.clone();

            let result =
                step_with_defined(&mut cpu, &mut memory, &mut Pio::default(), ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = if zero { RIP + 6 + 0xb4 } else { RIP + 6 };
            assert_eq!(cpu, expected, "ZF={zero}, defined={zf_defined}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
        }
    }
}

#[test]
fn rust_helper_jz_rel32_checks_canonicality_only_for_taken_target() {
    let code = [0x0f, 0x84, 0xb4, 0, 0, 0];
    let rip = 0x0000_7fff_ffff_fffa;
    let mut memory = Memory::new(&[]);
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffa..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.rflags &= !(1 << 6); // Not taken; noncanonical fall-through is deferred.
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before;
    expected.rip = 0x0000_8000_0000_0000;
    assert_eq!(cpu, expected);

    let rip = 0x0000_7fff_ffff_ff46;
    let mut memory = Memory::new(&[]);
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xf46..CODE_PHYSICAL + 0xf4c].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.rflags |= 1 << 6; // Taken target is the first noncanonical address.
    let before = cpu.clone();
    assert_eq!(rip + 6 + 0xb4, 0x0000_8000_0000_0000);

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

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
}

#[test]
fn rust_helper_jz_rel32_fetch_fault_on_last_displacement_byte_is_atomic() {
    let rip = RIP + 0xffb;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffb..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x0f, 0x84, 0xb4, 0, 0]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 5, page_fault(0x10, rip + 5)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 5),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_jz_rel32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0x84, 0xb3, 0, 0, 0], 0x84), // adjacent displacement
        (&[0x0f, 0x84, 0xb5, 0, 0, 0], 0x84), // adjacent displacement
        (&[0x0f, 0x85, 0xb4, 0, 0, 0], 0x85), // JNZ +0xb4
        (&[0x0f, 0x85, 0xb3, 0, 0, 0], 0x85), // different condition
        (&[0x66, 0x0f, 0x84, 0xb4, 0, 0, 0], 0x84), // operand-size prefix
        (&[0x40, 0x0f, 0x84, 0xb4, 0, 0, 0], 0x84), // REX prefix
        (&[0x74, 0xb4], 0x74),                // short JZ
        (&[0xe9, 0xb4, 0, 0, 0], 0xe9),       // JMP rel32
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
fn rust_helper_jz_rel32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x0f, 0x84, 0xb4, 0, 0, 0];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_jz_rel8_obeys_zf_and_preserves_flags_and_definedness() {
    let code = [0x74, 0x3b]; // JZ +0x3b
    for zero in [false, true] {
        for zf_defined in [false, true] {
            let defined = if zf_defined {
                DEFINED
            } else {
                DEFINED & !(1 << 6)
            };
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.rflags = (state().rflags & !(1 << 6)) | if zero { 1 << 6 } else { 0 };
            let before = cpu.clone();
            let expected_rip = if zero { RIP + 2 + 0x3b } else { RIP + 2 };
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = expected_rip;
            assert_eq!(cpu, expected, "ZF={zero}, defined={zf_defined}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_jz_rel8_checks_canonicality_only_when_taken() {
    let rip = 0x0000_7fff_ffff_fffe;
    let mut memory = Memory::new(&[]);
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x74, 0x3b]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.rflags &= !(1 << 6); // Not taken: noncanonical fall-through is deferred.
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.rip = 0x0000_8000_0000_0000;
    assert_eq!(cpu, expected);

    let mut memory = Memory::new(&[]);
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x74, 0x3b]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.rflags |= 1 << 6; // Taken target is noncanonical.
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

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
}

#[test]
fn rust_helper_jz_rel8_fetch_fault_is_atomic() {
    let rip = RIP + 0xfff;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x74;
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 1, page_fault(0x10, rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_jz_rel8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x74, 0x3a], 0x74),                // displacement below +0x3b
        (&[0x74, 0x3c], 0x74),                // displacement above +0x3b
        (&[0x75, 0x3b], 0x75),                // JNZ with same displacement
        (&[0x74, 0x24], 0x74),                // another firmware Jcc displacement
        (&[0x66, 0x74, 0x3b], 0x74),          // operand-size prefix
        (&[0x40, 0x74, 0x3b], 0x74),          // REX prefix
        (&[0x0f, 0x84, 0x3b, 0, 0, 0], 0x84), // near JZ
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_jz_rel8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x74, 0x3b];
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

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x74 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_seta_cl_obeys_cf_zf_truth_table_and_preserves_flags() {
    let code = [0x0f, 0x97, 0xc1]; // SETA CL
    let defined = DEFINED & !((1 << 0) | (1 << 6));
    for carry in [false, true] {
        for zero in [false, true] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.gpr[Register::Rcx as usize] = 0x1122_3344_5566_77aa;
            cpu.rflags = (state().rflags & !((1 << 0) | (1 << 6)))
                | if carry { 1 << 0 } else { 0 }
                | if zero { 1 << 6 } else { 0 };
            let before = cpu.clone();
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = RIP + code.len() as u64;
            expected.gpr[Register::Rcx as usize] =
                (expected.gpr[Register::Rcx as usize] & !0xff) | u64::from(!carry && !zero);
            assert_eq!(cpu, expected, "CF={carry}, ZF={zero}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(memory.write_addresses.is_empty());
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_seta_cl_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x0f, 0x97]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_seta_cl_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0x96, 0xc1], 0x96),       // SETBE CL
        (&[0x0f, 0x92, 0xc1], 0x92),       // SETB CL
        (&[0x0f, 0x97, 0xc0], 0x97),       // SETA AL
        (&[0x0f, 0x97, 0xc2], 0x97),       // SETA DL
        (&[0x0f, 0x97, 0xc9], 0x97),       // same CL, different ignored ModRM.reg
        (&[0x0f, 0x97, 0x01], 0x97),       // SETA byte ptr [RCX]
        (&[0x66, 0x0f, 0x97, 0xc1], 0x97), // operand-size prefix
        (&[0x40, 0x0f, 0x97, 0xc1], 0x97), // REX prefix
        (&[0x41, 0x0f, 0x97, 0xc1], 0x97), // REX.B changes destination
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_seta_cl_is_unsupported_in_cpl3_profile() {
    let bytes = [0x0f, 0x97, 0xc1];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_setae_dil_obeys_cf_and_preserves_flags() {
    let code = [0x40, 0x0f, 0x93, 0xc7]; // SETAE DIL
    let defined = DEFINED & !(1 << 0);
    for carry in [false, true] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_00aa;
        cpu.rflags = (state().rflags & !(1 << 0)) | u64::from(carry);
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.gpr[Register::Rdi as usize] =
            (expected.gpr[Register::Rdi as usize] & !0xff) | u64::from(!carry);
        assert_eq!(cpu, expected, "CF={carry}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_setae_dil_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x40, 0x0f, 0x93];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_setae_dil_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x40, 0x0f, 0x93, 0xc6], 0x93),       // SETAE SIL
        (&[0x40, 0x0f, 0x93, 0xcf], 0x93),       // same DIL, different ignored ModRM.reg
        (&[0x41, 0x0f, 0x93, 0xc7], 0x93),       // REX.B selects R15B
        (&[0x48, 0x0f, 0x93, 0xc7], 0x93),       // REX.W
        (&[0x0f, 0x93, 0xc7], 0x93),             // no REX selects BH
        (&[0x40, 0x0f, 0x93, 0xc0], 0x93),       // SETAE AL
        (&[0x40, 0x0f, 0x93, 0x07], 0x93),       // SETAE byte ptr [RDI]
        (&[0x66, 0x40, 0x0f, 0x93, 0xc7], 0x93), // operand-size prefix
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_setae_dil_is_unsupported_in_cpl3_profile() {
    let bytes = [0x40, 0x0f, 0x93, 0xc7];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x40 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_setae_r8b_obeys_cf_and_preserves_other_state() {
    let code = [0x41, 0x0f, 0x93, 0xc0]; // SETAE R8B
    for cf_defined in [false, true] {
        let defined = if cf_defined {
            DEFINED
        } else {
            DEFINED & !(1 << 0)
        };
        for carry in [false, true] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.gpr[Register::R8 as usize] = 0x1122_3344_5566_77aa;
            cpu.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0099;
            cpu.rflags = (state().rflags & !(1 << 0)) | u64::from(carry);
            let before = cpu.clone();
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = RIP + code.len() as u64;
            expected.gpr[Register::R8 as usize] =
                (expected.gpr[Register::R8 as usize] & !0xff) | u64::from(!carry);
            assert_eq!(cpu, expected, "CF={carry}, CF-defined={cf_defined}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(memory.write_addresses.is_empty());
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_setae_r8b_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x41, 0x0f, 0x93];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_setae_r8b_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0x0f, 0x92, 0xc0], 0x92),       // SETB R8B
        (&[0x41, 0x0f, 0x97, 0xc0], 0x97),       // SETA R8B
        (&[0x41, 0x0f, 0x93, 0xc1], 0x93),       // SETAE R9B
        (&[0x41, 0x0f, 0x93, 0xc8], 0x93),       // different ignored ModRM.reg
        (&[0x40, 0x0f, 0x93, 0xc0], 0x93),       // SETAE AL
        (&[0x49, 0x0f, 0x93, 0xc0], 0x93),       // REX.W+B
        (&[0x0f, 0x93, 0xc4], 0x93),             // no REX selects AH
        (&[0x41, 0x0f, 0x93, 0x00], 0x93),       // memory destination
        (&[0x66, 0x41, 0x0f, 0x93, 0xc0], 0x93), // operand-size prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_setae_r8b_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x0f, 0x93, 0xc0];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_setb_dil_obeys_cf_and_preserves_flags() {
    let code = [0x40, 0x0f, 0x92, 0xc7]; // SETB DIL
    for cf_defined in [false, true] {
        let defined = if cf_defined {
            DEFINED
        } else {
            DEFINED & !(1 << 0)
        };
        for carry in [false, true] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_00aa;
            cpu.rflags = (state().rflags & !(1 << 0)) | u64::from(carry);
            let before = cpu.clone();
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = RIP + code.len() as u64;
            expected.gpr[Register::Rdi as usize] =
                (expected.gpr[Register::Rdi as usize] & !0xff) | u64::from(carry);
            assert_eq!(cpu, expected, "CF={carry}, CF-defined={cf_defined}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(memory.write_addresses.is_empty());
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_setb_dil_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x40, 0x0f, 0x92];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_setb_dil_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x40, 0x0f, 0x96, 0xc7], 0x96),       // SETBE DIL
        (&[0x40, 0x0f, 0x92, 0xc6], 0x92),       // SETB SIL
        (&[0x40, 0x0f, 0x92, 0xcf], 0x92),       // different ignored ModRM.reg
        (&[0x41, 0x0f, 0x92, 0xc7], 0x92),       // REX.B selects R15B
        (&[0x48, 0x0f, 0x92, 0xc7], 0x92),       // REX.W
        (&[0x0f, 0x92, 0xc7], 0x92),             // no REX selects BH
        (&[0x40, 0x0f, 0x92, 0xc0], 0x92),       // SETB AL
        (&[0x40, 0x0f, 0x92, 0x07], 0x92),       // SETB byte ptr [RDI]
        (&[0x66, 0x40, 0x0f, 0x92, 0xc7], 0x92), // operand-size prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_setb_dil_is_unsupported_in_cpl3_profile() {
    let bytes = [0x40, 0x0f, 0x92, 0xc7];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x40 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_setz_dil_obeys_zf_and_preserves_flags() {
    let code = [0x40, 0x0f, 0x94, 0xc7]; // SETZ DIL
    for zf_defined in [false, true] {
        let defined = if zf_defined {
            DEFINED
        } else {
            DEFINED & !(1 << 6)
        };
        for zero in [false, true] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_00aa;
            cpu.rflags = (state().rflags & !(1 << 6)) | (u64::from(zero) << 6);
            let before = cpu.clone();
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = RIP + code.len() as u64;
            expected.gpr[Register::Rdi as usize] =
                (expected.gpr[Register::Rdi as usize] & !0xff) | u64::from(zero);
            assert_eq!(cpu, expected, "ZF={zero}, ZF-defined={zf_defined}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(memory.write_addresses.is_empty());
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_setz_dil_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x40, 0x0f, 0x94];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_setz_dil_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x40, 0x0f, 0x95, 0xc7], 0x95),       // SETNZ DIL
        (&[0x40, 0x0f, 0x94, 0xc6], 0x94),       // SETZ SIL
        (&[0x40, 0x0f, 0x94, 0xcf], 0x94),       // different ignored ModRM.reg
        (&[0x41, 0x0f, 0x94, 0xc7], 0x94),       // REX.B selects R15B
        (&[0x48, 0x0f, 0x94, 0xc7], 0x94),       // REX.W
        (&[0x0f, 0x94, 0xc7], 0x94),             // no REX selects BH
        (&[0x40, 0x0f, 0x94, 0xc0], 0x94),       // SETZ AL
        (&[0x40, 0x0f, 0x94, 0x07], 0x94),       // SETZ byte ptr [RDI]
        (&[0x66, 0x40, 0x0f, 0x94, 0xc7], 0x94), // operand-size prefix
        (&[0xf3, 0x40, 0x0f, 0x94, 0xc7], 0x94), // REP prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_setz_dil_is_unsupported_in_cpl3_profile() {
    let bytes = [0x40, 0x0f, 0x94, 0xc7];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x40 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_or_dil_cl_updates_byte_and_logic_flags() {
    let code = [0x40, 0x08, 0xcf]; // OR DIL,CL
    let defined = DEFINED | (1 << 4); // AF starts defined and must become undefined.
    let arithmetic_flags = 0x8d5;
    let logic_defined_flags = arithmetic_flags & !(1 << 4);
    for (dil, cl, result_byte, result_flags) in [
        (0x55, 0x0f, 0x5f, 0x04),
        (0x80, 0x7f, 0xff, 0x84),
        (0x00, 0x00, 0x00, 0x44),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0000 | dil;
        cpu.gpr[Register::Rcx as usize] = 0x1122_3344_5566_7700 | cl;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let step = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(step.exit, PagedProbeExit::Retired);
        assert_eq!(step.instructions_executed, 1);
        assert_eq!(
            step.rflags_defined,
            (defined & !(1 << 4)) | logic_defined_flags
        );
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.gpr[Register::Rdi as usize] =
            (expected.gpr[Register::Rdi as usize] & !0xff) | result_byte;
        expected.rflags = (expected.rflags & !(arithmetic_flags & !(1 << 4))) | result_flags;
        assert_eq!(cpu, expected, "DIL={dil:#04x}, CL={cl:#04x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_or_dil_cl_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x40, 0x08];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let step = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        step.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(step.instructions_executed, 0);
    assert_eq!(step.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_or_dil_cl_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x08, 0xcf], 0x08),             // without REX, destination is BH
        (&[0x40, 0x08, 0xce], 0x08),       // OR SIL,CL
        (&[0x40, 0x08, 0xc7], 0x08),       // OR DIL,AL
        (&[0x40, 0x0a, 0xf9], 0x0a),       // reverse direction
        (&[0x40, 0x20, 0xcf], 0x20),       // AND DIL,CL
        (&[0x40, 0x09, 0xcf], 0x09),       // dword OR EDI,ECX
        (&[0x41, 0x08, 0xcf], 0x08),       // REX.B selects R15B
        (&[0x48, 0x08, 0xcf], 0x08),       // REX.W
        (&[0x40, 0x08, 0x0f], 0x08),       // memory destination
        (&[0xf3, 0x40, 0x08, 0xcf], 0x08), // REP prefix
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let mut pio = Pio::default();
        let step = step(&mut cpu, &mut memory, &mut pio, ring0());

        assert_eq!(
            step.exit,
            PagedProbeExit::Unsupported { opcode: *opcode },
            "{bytes:02x?}"
        );
        assert_eq!(step.instructions_executed, 0);
        assert_eq!(step.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "{bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_or_dil_cl_is_unsupported_in_cpl3_profile() {
    let bytes = [0x40, 0x08, 0xcf];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    let before = cpu.clone();
    let step = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(step.exit, PagedProbeExit::Unsupported { opcode: 0x40 });
    assert_eq!(step.instructions_executed, 0);
    assert_eq!(step.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_and_r8b_dil_updates_only_r8b_and_logic_flags() {
    let code = [0x41, 0x20, 0xf8]; // AND R8B,DIL
    let arithmetic_flags = 0x8d5;
    let logic_defined_flags = arithmetic_flags & !(1 << 4);
    let defined = DEFINED | (1 << 4); // AF starts defined and becomes undefined.
    for (r8b, dil, result_byte, result_flags) in [
        (0x55, 0x0f, 0x05, 0x04),
        (0x80, 0x7f, 0x00, 0x44),
        (0xff, 0x81, 0x81, 0x84),
        (0x00, 0xf0, 0x00, 0x44),
    ] {
        assert_eq!(r8b & dil, result_byte);
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R8 as usize] = 0x1122_3344_5566_7700 | r8b;
        cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0000 | dil;
        cpu.gpr[Register::Rax as usize] = 0xfedc_ba98_7654_3210;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            result.rflags_defined,
            (defined & !(1 << 4)) | logic_defined_flags
        );
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.gpr[Register::R8 as usize] =
            (expected.gpr[Register::R8 as usize] & !0xff) | result_byte;
        expected.rflags = (expected.rflags & !(arithmetic_flags & !(1 << 4))) | result_flags;
        assert_eq!(cpu, expected, "R8B={r8b:#04x}, DIL={dil:#04x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_and_r8b_dil_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x41, 0x20];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_and_r8b_dil_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0x20, 0xf0], 0x20),       // AND R8B,SIL
        (&[0x41, 0x08, 0xf8], 0x08),       // OR R8B,DIL
        (&[0x44, 0x20, 0xc7], 0x20),       // reverse direction: AND DIL,R8B
        (&[0x41, 0x20, 0xf9], 0x20),       // AND R9B,DIL
        (&[0x49, 0x20, 0xf8], 0x20),       // REX.W+B
        (&[0x40, 0x20, 0xf8], 0x20),       // AND AL,DIL
        (&[0x20, 0xfc], 0x20),             // no REX
        (&[0x41, 0x20, 0x00], 0x20),       // memory destination
        (&[0x66, 0x41, 0x20, 0xf8], 0x20), // operand-size prefix
        (&[0xf3, 0x41, 0x20, 0xf8], 0x20), // REP prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_and_r8b_dil_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x20, 0xf8];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_and_r15_r8_updates_only_r15_and_logic_flags() {
    let code = [0x4d, 0x21, 0xc7]; // AND R15,R8
    let arithmetic_flags = 0x8d5;
    let defined = DEFINED | (1 << 4); // AF starts defined and becomes undefined.
    for (r15, r8, result_value, result_flags) in [
        (
            0x8000_0000_0000_0080,
            0x8000_0000_0000_0080,
            0x8000_0000_0000_0080,
            0x80,
        ),
        (
            0x0000_0000_0000_00ff,
            0x0000_0000_0000_0003,
            0x0000_0000_0000_0003,
            0x04,
        ),
        (
            0x0000_0000_0000_0000,
            0xffff_ffff_ffff_ffff,
            0x0000_0000_0000_0000,
            0x44,
        ),
        (
            0x0000_0000_0000_0001,
            0x0000_0000_0000_0001,
            0x0000_0000_0000_0001,
            0x00,
        ),
    ] {
        assert_eq!(r15 & r8, result_value);
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R15 as usize] = r15;
        cpu.gpr[Register::R8 as usize] = r8;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            result.rflags_defined,
            (defined & !0x10) | (arithmetic_flags & !0x10)
        );
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.gpr[Register::R15 as usize] = result_value;
        expected.rflags = (expected.rflags & !(arithmetic_flags & !0x10)) | result_flags;
        assert_eq!(cpu, expected, "R15={r15:#018x}, R8={r8:#018x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_and_r15_r8_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let code = [0x4d, 0x21];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_and_r15_r8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x4d, 0x21, 0xc6], 0x21),       // AND R14,R8
        (&[0x4d, 0x21, 0xcf], 0x21),       // AND R15,R9
        (&[0x49, 0x21, 0xc7], 0x21),       // AND R15,RAX
        (&[0x4d, 0x23, 0xc7], 0x23),       // reverse direction: AND R8,R15
        (&[0x45, 0x21, 0xc7], 0x21),       // dword AND R15D,R8D
        (&[0x4d, 0x21, 0x07], 0x21),       // memory destination
        (&[0x4c, 0x21, 0xc7], 0x21),       // different REX.B/destination
        (&[0x66, 0x4d, 0x21, 0xc7], 0x21), // operand-size prefix
        (&[0xf3, 0x4d, 0x21, 0xc7], 0x21), // REP prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_and_r15_r8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x4d, 0x21, 0xc7];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x4d });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_test_r8b_dil_updates_only_logic_flags() {
    let code = [0x41, 0x84, 0xf8]; // TEST R8B,DIL
    let arithmetic_flags = 0x8d5;
    let logic_defined_flags = arithmetic_flags & !(1 << 4);
    let defined = DEFINED | (1 << 4); // AF starts defined and becomes undefined.
    for (r8b, dil, result_byte, result_flags) in [
        (0x55, 0x0f, 0x05, 0x04),
        (0x80, 0x7f, 0x00, 0x44),
        (0xff, 0x81, 0x81, 0x84),
        (0x00, 0xf0, 0x00, 0x44),
    ] {
        assert_eq!(r8b & dil, result_byte);
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R8 as usize] = 0x1122_3344_5566_7700 | r8b;
        cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0000 | dil;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();

        let result =
            step_with_defined(&mut cpu, &mut memory, &mut Pio::default(), ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            result.rflags_defined,
            (defined & !(1 << 4)) | logic_defined_flags
        );
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.rflags = (expected.rflags & !logic_defined_flags) | result_flags;
        assert_eq!(cpu, expected, "R8B={r8b:#04x}, DIL={dil:#04x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
    }
}

#[test]
fn rust_helper_test_r8b_dil_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let code = [0x41, 0x84];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_test_r8b_dil_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0x84, 0xf0], 0x84),       // TEST R8B,SIL
        (&[0x44, 0x84, 0xc7], 0x84),       // reverse operands: TEST DIL,R8B
        (&[0x41, 0x20, 0xf0], 0x20),       // AND R8B,SIL
        (&[0x44, 0x84, 0xf8], 0x84),       // REX.R selects R15B
        (&[0x49, 0x84, 0xf8], 0x84),       // REX.W+B
        (&[0x40, 0x84, 0xf8], 0x84),       // TEST AL,DIL
        (&[0x84, 0xf8], 0x84),             // no REX
        (&[0x41, 0x84, 0x38], 0x84),       // memory destination
        (&[0x66, 0x41, 0x84, 0xf8], 0x84), // operand-size prefix
        (&[0xf3, 0x41, 0x84, 0xf8], 0x84), // REP prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_test_r8b_dil_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x84, 0xf8];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_test_bpl_imm8_updates_only_logic_flags() {
    let code = [0x40, 0xf6, 0xc5, 0x07]; // TEST BPL,7
    let arithmetic_flags = 0x8d5;
    let logic_defined_flags = arithmetic_flags & !(1 << 4);
    let defined = DEFINED | (1 << 4); // AF starts defined and becomes undefined.
    for bpl in [0x00, 0x01, 0x03, 0x07, 0x08, 0x80, 0xff] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rbp as usize] = 0x1122_3344_5566_7700 | bpl;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let result_byte = bpl & 7;
        let mut result_flags = if result_byte == 0 { 1 << 6 } else { 0 };
        if result_byte & 0x80 != 0 {
            result_flags |= 1 << 7;
        }
        if result_byte.count_ones() % 2 == 0 {
            result_flags |= 1 << 2;
        }

        let result =
            step_with_defined(&mut cpu, &mut memory, &mut Pio::default(), ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            result.rflags_defined,
            (defined & !(1 << 4)) | logic_defined_flags
        );
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.rflags = (expected.rflags & !logic_defined_flags) | result_flags;
        assert_eq!(cpu, expected, "BPL={bpl:#04x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
    }
}

#[test]
fn rust_helper_test_bpl_imm8_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x40, 0xf6, 0xc5];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_test_bpl_imm8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x40, 0xf6, 0xc3, 0x07], 0xf6),          // TEST BL,7
        (&[0x40, 0xf6, 0xc5, 0x06], 0xf6),          // immediate 6
        (&[0x40, 0xf6, 0xc5, 0x80], 0xf6),          // immediate 0x80
        (&[0xf6, 0xc5, 0x07], 0xf6),                // no REX: TEST CH,7
        (&[0x41, 0xf6, 0xc5, 0x07], 0xf6),          // R13B
        (&[0x48, 0xf6, 0xc5, 0x07], 0xf6),          // REX.W
        (&[0x66, 0x40, 0xf6, 0xc5, 0x07], 0xf6),    // operand-size prefix
        (&[0xf3, 0x40, 0xf6, 0xc5, 0x07], 0xf6),    // REP prefix
        (&[0x40, 0x84, 0xc5], 0x84),                // TEST BPL,AL
        (&[0x40, 0xf6, 0x05, 0, 0, 0, 0, 7], 0xf6), // memory operand
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_test_bpl_imm8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x40, 0xf6, 0xc5, 0x07];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x40 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_ja_rel8_obeys_cf_zf_truth_table_and_preserves_definedness() {
    let code = [0x77, 0x0e]; // JA/JNBE +0x0e
    let defined = DEFINED & !((1 << 0) | (1 << 6));
    for carry in [false, true] {
        for zero in [false, true] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.rflags = 0x2 | (1 << 9) | (1 << 10) | (1 << 18);
            if carry {
                cpu.rflags |= 1;
            }
            if zero {
                cpu.rflags |= 1 << 6;
            }
            let before = cpu.clone();
            let mut pio = Pio::default();
            let taken = !carry && !zero;

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.rip = if taken { RIP + 2 + 0xe } else { RIP + 2 };
            assert_eq!(cpu, expected, "CF={carry}, ZF={zero}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(memory.write_addresses.is_empty());
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_helper_ja_rel8_checks_canonicality_only_when_taken() {
    let start = 0x0000_7fff_ffff_fffe;
    let code = [0x77, 0x0e];
    let defined = DEFINED & !((1 << 0) | (1 << 6));

    // CF=1 makes the condition false. Fall-through is noncanonical but is
    // not checked by a not-taken conditional branch.
    let mut memory = Memory::new(&[]);
    memory.pages.insert(start & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = start;
    cpu.rflags |= 1;
    cpu.rflags &= !(1 << 6);
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);
    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.rip = 0x0000_8000_0000_0000;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));

    // CF=ZF=0 takes the branch; its target is noncanonical and faults #GP.
    let mut memory = Memory::new(&[]);
    memory.pages.insert(start & !0xfff, CODE_PHYSICAL);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = start;
    cpu.rflags &= !((1 << 0) | (1 << 6));
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, defined);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_ja_rel8_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x77;
    let mut cpu = state();
    cpu.rip = RIP + 0xfff;
    cpu.gpr = gpr_patterns();
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
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_ja_rel8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x77, 0x0d], 0x77),                // different displacement
        (&[0x77, 0x0f], 0x77),                // different displacement
        (&[0x76, 0x0e], 0x76),                // JBE
        (&[0x73, 0x0e], 0x73),                // JAE
        (&[0x0f, 0x87, 0x0e, 0, 0, 0], 0x87), // near JA
        (&[0x66, 0x77, 0x0e], 0x77),          // operand-size prefix
        (&[0x40, 0x77, 0x0e], 0x77),          // REX prefix
        (&[0xeb, 0x0e], 0xeb),                // JMP rel8
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_ja_rel8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x77, 0x0e];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x77 });
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
fn firmware_entry_mov_rdi_r8_copies_only_rdi_and_advances_rip() {
    let code = [0x4c, 0x89, 0xc7]; // MOV RDI,R8
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R8 as usize] = 0x1122_3344_5566_7788;
    cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::Rdi as usize] = expected.gpr[Register::R8 as usize];
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_mov_rdi_r8_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x4c, 0x89]);
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
fn firmware_entry_mov_rdi_r8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x4c, 0x89, 0xcf], 0x89),       // MOV RDI,R9
        (&[0x4c, 0x89, 0xc6], 0x89),       // MOV RSI,R8
        (&[0x4c, 0x89, 0xc0], 0x89),       // MOV RAX,R8
        (&[0x44, 0x89, 0xc7], 0x89),       // dword MOV EDI,R8D
        (&[0x49, 0x8b, 0xf8], 0x8b),       // alternate opcode for MOV RDI,R8
        (&[0x4c, 0x89, 0x07], 0x89),       // MOV [RDI],R8
        (&[0x66, 0x4c, 0x89, 0xc7], 0x89), // extra operand-size prefix
        (&[0x40, 0x4c, 0x89, 0xc7], 0x89), // redundant REX
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
fn firmware_entry_mov_rdi_r8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x4c, 0x89, 0xc7];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x4c });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_rbx_rdi_copies_only_rbx_and_preserves_flags() {
    let code = [0x48, 0x89, 0xfb]; // MOV RBX,RDI
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rdi as usize] = 0x1122_3344_5566_7788;
    cpu.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::Rbx as usize] = expected.gpr[Register::Rdi as usize];
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_rbx_rdi_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x48, 0x89]);
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
fn rust_helper_mov_rbx_rdi_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x89, 0xf9], 0x89),       // MOV RCX,RDI
        (&[0x48, 0x89, 0xdf], 0x89),       // MOV RDI,RBX
        (&[0x49, 0x89, 0xfb], 0x89),       // MOV R11,RDI
        (&[0x48, 0x89, 0xdb], 0x89),       // MOV RBX,RBX
        (&[0x89, 0xfb], 0x89),             // dword MOV EBX,EDI
        (&[0x48, 0x8b, 0xdf], 0x8b),       // alternate opcode
        (&[0x48, 0x89, 0x3f], 0x89),       // memory destination
        (&[0x66, 0x48, 0x89, 0xfb], 0x89), // extra operand-size prefix
        (&[0x4c, 0x89, 0xfb], 0x89),       // different REX
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_mov_rbx_rdi_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x89, 0xfb];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xfb });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_rax_rdi_copies_only_rax_and_preserves_flags() {
    let code = [0x48, 0x89, 0xf8]; // MOV RAX,RDI
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rdi as usize] = 0x1122_3344_5566_7788;
    cpu.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::Rax as usize] = expected.gpr[Register::Rdi as usize];
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_rax_rdi_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x48, 0x89]);
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
fn rust_helper_mov_rax_rdi_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x89, 0xf9], 0x89),       // MOV RCX,RDI
        (&[0x48, 0x89, 0xc7], 0x89),       // MOV RDI,RAX
        (&[0x4c, 0x89, 0xf8], 0x89),       // MOV RAX,R15 (different REX/source)
        (&[0x48, 0x89, 0xc0], 0x89),       // MOV RAX,RAX
        (&[0x89, 0xf8], 0x89),             // dword MOV EAX,EDI
        (&[0x48, 0x8b, 0xc7], 0x8b),       // alternate opcode
        (&[0x48, 0x89, 0x07], 0x89),       // memory destination
        (&[0x66, 0x48, 0x89, 0xf8], 0x89), // extra operand-size prefix
        (&[0x40, 0x48, 0x89, 0xf8], 0x89), // redundant REX prefix
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_mov_rax_rdi_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x89, 0xf8];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xf8 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_r15_r11_copies_only_r15_and_preserves_flags() {
    let code = [0x4d, 0x89, 0xdf]; // MOV R15,R11
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R11 as usize] = 0x8123_4567_89ab_cdef;
    cpu.gpr[Register::R15 as usize] = 0x0123_4567_89ab_cdef;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::R15 as usize] = expected.gpr[Register::R11 as usize];
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_r15_r11_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffe..CODE_PHYSICAL + 0x1000].copy_from_slice(&[0x4d, 0x89]);
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
fn rust_helper_mov_r15_r11_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x4d, 0x89, 0xd7], 0x89),       // MOV R15,R10
        (&[0x4d, 0x89, 0xde], 0x89),       // MOV R14,R11
        (&[0x49, 0x89, 0xdf], 0x89),       // different REX/source register
        (&[0x4d, 0x89, 0xf7], 0x89),       // MOV R15,R14
        (&[0x4d, 0x8b, 0xfb], 0x8b),       // alternate opcode/direction
        (&[0x45, 0x89, 0xdf], 0x89),       // dword MOV R15D,R11D
        (&[0x4c, 0x89, 0xdf], 0x89),       // different REX.W/R
        (&[0x66, 0x4d, 0x89, 0xdf], 0x89), // extra operand-size prefix
        (&[0x4d, 0x89, 0x3b], 0x89),       // memory destination
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_mov_r15_r11_is_unsupported_in_cpl3_profile() {
    let bytes = [0x4d, 0x89, 0xdf];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x4d });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn firmware_entry_call_rel32_pushes_return_rip_and_branches() {
    let code = [0xe8, 0x83, 0x01, 0, 0]; // CALL rel32 to od_fw_main
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let return_rip = RIP + code.len() as u64;
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] -= 8;
    expected.rip = return_rip + 0x183;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        return_rip.to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
    // In the built firmware, CALL at 0x200068 targets 0x2001f0.
    assert_eq!(0x200068u64 + 5 + 0x183, 0x2001f0);
}

#[test]
fn rust_entry_call_rel32_pushes_return_rip_and_targets_handoff_helper() {
    let code = [0xe8, 0x8d, 0x02, 0x00, 0x00]; // CALL 0x2004a0 from 0x20020e
    let rip = 0x20020e;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let return_rip = rip + code.len() as u64;
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] -= 8;
    expected.rip = 0x2004a0;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        return_rip.to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_call_rel32_0x859_pushes_return_rip_and_targets_helper() {
    let code = [0xe8, 0x59, 0x08, 0, 0]; // CALL 0x200d10 from 0x2004b2
    let rip = 0x2004b2;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let return_rip = rip + code.len() as u64;
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] -= 8;
    expected.rip = 0x200d10;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        return_rip.to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
    assert_eq!(rip + code.len() as u64 + 0x859, 0x200d10);
}

#[test]
fn rust_helper_call_rel32_0x859_faults_are_atomic() {
    let code = [0xe8, 0x59, 0x08, 0, 0];
    let rip = 0x2004b2;

    // Stack write fault: no RSP/RIP/flags commit.
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, ring0());
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);

    // Noncanonical stack pointer raises #SS without changing CPU state.
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let result = step(&mut cpu, &mut memory, &mut pio, ring0());
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);

    // A noncanonical target takes #GP before the independently invalid stack.
    let noncanonical_target_rip = 0x0000_7fff_ffff_f7a2;
    let mut memory = memory_with_code_at(&code, noncanonical_target_rip);
    let mut cpu = state();
    cpu.rip = noncanonical_target_rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();
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
    assert_eq!(memory.writes, 0);

    // Instruction fetch fault before the complete rel32 is fetched.
    let fetch_rip = RIP + 0xffd;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffd..CODE_PHYSICAL + 0x1000].copy_from_slice(&code[..3]);
    let mut cpu = state();
    cpu.rip = fetch_rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((fetch_rip + 3, page_fault(0x10, fetch_rip + 3)));
    let result = step(&mut cpu, &mut memory, &mut pio, ring0());
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(fetch_rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_call_stack_write_fault_is_atomic() {
    let code = [0xe8, 0x8d, 0x02, 0x00, 0x00];
    let rip = 0x20020e;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_call_noncanonical_stack_fault_is_atomic() {
    let code = [0xe8, 0x8d, 0x02, 0x00, 0x00];
    let rip = 0x20020e;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_call_noncanonical_target_precedes_invalid_stack() {
    let code = [0xe8, 0x8d, 0x02, 0x00, 0x00];
    let rip = 0x0000_7fff_ffff_fd6e;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

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
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_call_fetch_fault_does_not_commit() {
    let code = [0xe8, 0x8d, 0x02, 0x00, 0x00];
    let rip = RIP + 0xffd;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_call_stack_write_fault_is_atomic() {
    let code = [0xe8, 0x83, 0x01, 0, 0];
    let mut memory = Memory::new(&code);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_call_noncanonical_stack_fault_is_atomic() {
    let code = [0xe8, 0x83, 0x01, 0, 0];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    // RSP is decremented before the stack address is checked; choose a value
    // whose resulting RSP - 8 is noncanonical.
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_call_noncanonical_target_precedes_invalid_stack() {
    let code = [0xe8, 0x83, 0x01, 0, 0];
    let mut memory = Memory::new(&[]);
    let rip = 0x0000_7fff_ffff_fffb;
    memory.pages.insert(rip & !0xfff, CODE_PHYSICAL);
    let code_offset = (rip & 0xfff) as usize;
    memory.bytes[CODE_PHYSICAL + code_offset..CODE_PHYSICAL + code_offset + code.len()]
        .copy_from_slice(&code);
    let mut cpu = state();
    cpu.rip = rip;
    // Both the CALL target and the decremented stack pointer are noncanonical.
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

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
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_call_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffd..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0xe8, 0x83, 0x01]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffd;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 3, page_fault(0x10, cpu.rip + 3)));

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
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_call_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0xe8, 0x82, 0x01, 0, 0], 0xe8), // different target displacement
        (&[0xe8, 0x83, 0x00, 0, 0], 0xe8), // different target displacement
        (&[0xe8, 0x8c, 0x02, 0, 0], 0xe8), // adjacent Rust-call displacement
        (&[0xe8, 0x58, 0x08, 0, 0], 0xe8), // adjacent displacement below Rust-helper call
        (&[0xe8, 0x5a, 0x08, 0, 0], 0xe8), // adjacent displacement above Rust-helper call
        (&[0xe9, 0x83, 0x01, 0, 0], 0xe9), // JMP rel32
        (&[0x2e, 0xe8, 0x83, 0x01, 0, 0], 0xe8), // null-base segment prefix
        (&[0x66, 0xe8, 0x83, 0x01, 0, 0], 0xe8), // operand-size prefix
        (&[0x67, 0xe8, 0x83, 0x01, 0, 0], 0xe8), // address-size prefix
        (&[0xf3, 0xe8, 0x83, 0x01, 0, 0], 0xe8), // REP prefix
        (&[0xff, 0xd0], 0xff),             // indirect CALL RAX
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn firmware_entry_call_is_unsupported_in_cpl3_profile() {
    for bytes in [
        [0xe8, 0x83, 0x01, 0, 0],
        [0xe8, 0x8d, 0x02, 0, 0],
        [0xe8, 0x59, 0x08, 0, 0],
    ] {
        let mut memory = Memory::new(&bytes);
        let mut cpu = state();
        cpu.cr3 = Some(0x1000);
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
        let before = cpu.clone();
        let result = execute::step_paged_cpl3_mov(
            &mut cpu,
            &mut memory,
            context(InstructionMode::Long64, PrivilegeLevel::Ring3),
            0xd00,
            DEFINED,
        );
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0xe8 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn rust_helper_push_r14_stores_qword_and_updates_only_rsp_rip() {
    let code = [0x41, 0x56];
    let rip = 0x2004a0;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.gpr[Register::R14 as usize] = 0x1122_3344_5566_7788;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rsp as usize] = DATA + 0xf8;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        before.gpr[Register::R14 as usize].to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r14_stack_page_fault_is_atomic() {
    let code = [0x41, 0x56];
    let rip = 0x2004a0;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r14_noncanonical_stack_fault_is_atomic() {
    let code = [0x41, 0x56];
    let rip = 0x2004a0;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r14_fetch_fault_does_not_commit() {
    let code = [0x41, 0x56];
    let rip = RIP + 0xfff;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 1, page_fault(0x10, rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r14_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x56], 0x56),             // PUSH RSI without REX.B
        (&[0x41, 0x53], 0x53),       // PUSH R11
        (&[0x49, 0x56], 0x56),       // redundant REX.W
        (&[0x66, 0x41, 0x56], 0x56), // word PUSH R14
        (&[0x40, 0x41, 0x56], 0x56), // redundant REX prefix
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_push_r14_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x56];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn rust_helper_push_rbx_stores_qword_and_updates_only_rsp_rip() {
    let code = [0x53];
    let rip = 0x2004a2;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.gpr[Register::Rbx as usize] = 0x1122_3344_5566_7788;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rsp as usize] = DATA + 0xf8;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        before.gpr[Register::Rbx as usize].to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbx_stack_page_fault_is_atomic() {
    let code = [0x53];
    let rip = 0x2004a2;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbx_noncanonical_stack_fault_is_atomic() {
    let code = [0x53];
    let rip = 0x2004a2;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbx_fetch_fault_does_not_commit() {
    let rip = RIP + 0xfff;
    let mut memory = memory_with_code_at(&[0x53], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip, page_fault(0x10, rip)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbx_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0x53], 0x53), // PUSH R11
        (&[0x40, 0x53], 0x53), // redundant REX prefix
        (&[0x66, 0x53], 0x53), // word PUSH BX
        (&[0x52], 0x52),       // PUSH RDX
        (&[0x51], 0x51),       // PUSH RCX from another bounded stack profile
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_push_rbx_is_unsupported_in_cpl3_profile() {
    let bytes = [0x53];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x53 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn rust_helper_push_rbp_stores_qword_and_updates_only_rsp_rip() {
    let code = [0x55];
    let rip = 0x200d39;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.gpr[Register::Rbp as usize] = 0x8877_6655_4433_2211;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rsp as usize] = DATA + 0xf8;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        before.gpr[Register::Rbp as usize].to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbp_stack_page_fault_is_atomic() {
    let code = [0x55];
    let rip = 0x200d39;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbp_noncanonical_stack_fault_is_atomic() {
    let code = [0x55];
    let rip = 0x200d39;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbp_fetch_fault_does_not_commit() {
    let rip = RIP + 0xfff;
    let mut memory = memory_with_code_at(&[0x55], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip, page_fault(0x10, rip)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_rbp_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x54], 0x54),       // PUSH RSP
        (&[0x56], 0x56),       // PUSH RSI
        (&[0x57], 0x57),       // PUSH RDI
        (&[0x40, 0x55], 0x55), // redundant REX prefix
        (&[0x66, 0x55], 0x55), // word PUSH BP
        (&[0x48, 0x55], 0x55), // REX.W prefix
        (&[0xf3, 0x55], 0x55), // REP prefix
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_push_rbp_is_unsupported_in_cpl3_profile() {
    let bytes = [0x55];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x55 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn rust_helper_push_r15_stores_qword_and_updates_only_rsp_rip() {
    let code = [0x41, 0x57];
    let rip = 0x200d3a;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.gpr[Register::R15 as usize] = 0x8877_6655_4433_2211;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rsp as usize] = DATA + 0xf8;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        before.gpr[Register::R15 as usize].to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r15_stack_page_fault_is_atomic() {
    let code = [0x41, 0x57];
    let rip = 0x200d3a;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r15_noncanonical_stack_fault_is_atomic() {
    let code = [0x41, 0x57];
    let rip = 0x200d3a;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r15_fetch_fault_does_not_commit() {
    let rip = RIP + 0xffe;
    let mut memory = memory_with_code_at(&[0x41, 0x57], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 1, page_fault(0x10, rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r15_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x57], 0x57),             // PUSH RDI
        (&[0x49, 0x57], 0x57),       // REX.W + REX.B
        (&[0x4d, 0x57], 0x57),       // REX.W/R/X/B
        (&[0x40, 0x57], 0x57),       // redundant REX prefix
        (&[0x66, 0x41, 0x57], 0x57), // word PUSH R15
        (&[0xf3, 0x41, 0x57], 0x57), // REP prefix
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_push_r15_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x57];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn rust_helper_push_r13_stores_qword_and_updates_only_rsp_rip() {
    let code = [0x41, 0x55];
    let rip = 0x200d3e;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.gpr[Register::R13 as usize] = 0x8877_6655_4433_2211;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rsp as usize] = DATA + 0xf8;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        before.gpr[Register::R13 as usize].to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r13_stack_page_fault_is_atomic() {
    let code = [0x41, 0x55];
    let rip = 0x200d3e;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r13_noncanonical_stack_fault_is_atomic() {
    let code = [0x41, 0x55];
    let rip = 0x200d3e;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r13_fetch_fault_does_not_commit() {
    let rip = RIP + 0xffe;
    let mut memory = memory_with_code_at(&[0x41, 0x55], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 1, page_fault(0x10, rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r13_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x49, 0x55], 0x55),       // REX.W + REX.B
        (&[0x4d, 0x55], 0x55),       // REX.W/R/X/B
        (&[0x40, 0x55], 0x55),       // redundant REX prefix
        (&[0x66, 0x41, 0x55], 0x55), // word PUSH R13
        (&[0xf3, 0x41, 0x55], 0x55), // REP prefix
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_push_r13_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x55];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn rust_helper_push_r12_stores_qword_and_updates_only_rsp_rip() {
    let code = [0x41, 0x54];
    let rip = 0x200d40;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    cpu.gpr[Register::R12 as usize] = 0x8877_6655_4433_2211;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rsp as usize] = DATA + 0xf8;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(
        memory.bytes[DATA_PHYSICAL + 0xf8..DATA_PHYSICAL + 0x100],
        before.gpr[Register::R12 as usize].to_le_bytes()
    );
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r12_stack_page_fault_is_atomic() {
    let code = [0x41, 0x54];
    let rip = 0x200d40;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_write = Some(page_fault(0x2, DATA + 0xf8));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x2),
            linear_address: Some(DATA + 0xf8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.write_addresses, [DATA + 0xf8]);
    assert_eq!(memory.reads, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r12_noncanonical_stack_fault_is_atomic() {
    let code = [0x41, 0x54];
    let rip = 0x200d40;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r12_fetch_fault_does_not_commit() {
    let rip = RIP + 0xffe;
    let mut memory = memory_with_code_at(&[0x41, 0x54], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 1, page_fault(0x10, rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_push_r12_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0x52], 0x52),       // PUSH R10
        (&[0x41, 0x53], 0x53),       // PUSH R11
        (&[0x49, 0x54], 0x54),       // REX.W + REX.B
        (&[0x4d, 0x54], 0x54),       // REX.W/R/X/B
        (&[0x40, 0x54], 0x54),       // redundant REX prefix
        (&[0x66, 0x41, 0x54], 0x54), // word PUSH R12
        (&[0xf3, 0x41, 0x54], 0x54), // REP prefix
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
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
        assert_eq!(cpu, before);
        assert_eq!(memory.writes, 0);
        assert_eq!(memory.reads, 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_push_r12_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0x54];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.writes, 0);
    assert_eq!(memory.reads, 0);
}

#[test]
fn rust_entry_sub_rsp_imm8_reserves_exact_stack_frame_and_updates_flags() {
    let code = [0x48, 0x83, 0xec, 0x58]; // SUB RSP,0x58
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = 0x14_ffa0;
    expected.rip = RIP + code.len() as u64;
    // 0x14_fff8 - 0x58 = 0x14_ffa0: PF is set; other arithmetic flags clear.
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_sub_rsp_imm8_does_not_validate_noncanonical_result() {
    let code = [0x48, 0x83, 0xec, 0x58];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0058;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    expected.rip = RIP + code.len() as u64;
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_sub_rsp_imm8_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffd..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x83, 0xec]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffd;
    cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 3, page_fault(0x10, cpu.rip + 3)));

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
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_sub_rsp_imm8_38_reserves_exact_frame_and_updates_flags() {
    let code = [0x48, 0x83, 0xec, 0x38]; // SUB RSP,0x38
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = 0x14_ffc0;
    expected.rip = RIP + code.len() as u64;
    // 0x14_fff8 - 0x38 = 0x14_ffc0: PF is set; other arithmetic flags clear.
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_sub_rsp_imm8_38_does_not_validate_noncanonical_result() {
    let code = [0x48, 0x83, 0xec, 0x38];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0038;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    expected.rip = RIP + code.len() as u64;
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_sub_rsp_imm8_38_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffd..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x83, 0xec]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffd;
    cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 3, page_fault(0x10, cpu.rip + 3)));

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
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_sub_rsp_imm8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x83, 0xec, 0x37], 0x83),          // neighboring immediate
        (&[0x48, 0x83, 0xec, 0x39], 0x83),          // neighboring immediate
        (&[0x48, 0x83, 0xec, 0x57], 0x83),          // different immediate
        (&[0x48, 0x83, 0xec, 0x59], 0x83),          // different immediate
        (&[0x48, 0x83, 0xc4, 0x58], 0x83),          // ADD RSP,0x58
        (&[0x48, 0x83, 0xfc, 0x58], 0x83),          // CMP RSP,0x58
        (&[0x48, 0x83, 0xed, 0x58], 0x83),          // SUB RBP,0x58
        (&[0x83, 0xec, 0x58], 0x83),                // dword SUB ESP,0x58
        (&[0x48, 0x81, 0xec, 0x58, 0, 0, 0], 0x81), // immediate dword form
        (&[0x48, 0x29, 0xec], 0x29),                // SUB RSP,RBP
        (&[0x48, 0x83, 0x6c, 0x24, 0x58, 0x58], 0x83), // memory destination
        (&[0x66, 0x48, 0x83, 0xec, 0x58], 0x83),    // operand-size prefix
        (&[0x67, 0x48, 0x83, 0xec, 0x58], 0x83),    // address-size prefix
        (&[0xf3, 0x48, 0x83, 0xec, 0x58], 0x83),    // REP prefix
        (&[0x2e, 0x48, 0x83, 0xec, 0x58], 0x83),    // segment prefix
        (&[0x4c, 0x83, 0xec, 0x58], 0x83),          // different REX
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_entry_sub_rsp_imm8_is_unsupported_in_cpl3_profile() {
    for bytes in [&[0x48, 0x83, 0xec, 0x58][..], &[0x48, 0x83, 0xec, 0x38][..]] {
        let mut memory = Memory::new(bytes);
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
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x83 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn rust_entry_sub_rsp_imm32_reserves_exact_stack_frame_and_updates_flags() {
    let code = [0x48, 0x81, 0xec, 0xb8, 0, 0, 0]; // SUB RSP,0xb8
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = 0x14_ff40;
    expected.rip = RIP + code.len() as u64;
    // 0x14_fff8 - 0xb8 = 0x14_ff40; all six arithmetic flags clear.
    expected.rflags &= !0x8d5;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_sub_rsp_imm32_does_not_validate_noncanonical_result() {
    let code = [0x48, 0x81, 0xec, 0xb8, 0, 0, 0];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr[Register::Rsp as usize] = 0x0000_8000_0000_00b8;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    expected.rip = RIP + code.len() as u64;
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_sub_rsp_imm32_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffc..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x81, 0xec, 0xb8]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffc;
    cpu.gpr[Register::Rsp as usize] = 0x14_fff8;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 4, page_fault(0x10, cpu.rip + 4)));

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
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_sub_rsp_imm32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x81, 0xec, 0xb7, 0, 0, 0], 0x81), // different immediate
        (&[0x48, 0x81, 0xec, 0xb9, 0, 0, 0], 0x81), // different immediate
        (&[0x48, 0x81, 0xc4, 0xb8, 0, 0, 0], 0x81), // ADD RSP,0xb8
        (&[0x48, 0x81, 0xfc, 0xb8, 0, 0, 0], 0x81), // CMP RSP,0xb8
        (&[0x48, 0x81, 0xed, 0xb8, 0, 0, 0], 0x81), // SUB RBP,0xb8
        (&[0x81, 0xec, 0xb8, 0, 0, 0], 0x81),       // dword SUB ESP,0xb8
        (&[0x48, 0x83, 0xec, 0xb8], 0x83),          // imm8 encoding, different operation
        (&[0x48, 0x29, 0xec], 0x29),                // SUB RSP,RBP
        (&[0x48, 0x81, 0x6c, 0x24, 0x58, 0xb8, 0, 0, 0], 0x81), // memory destination
        (&[0x66, 0x48, 0x81, 0xec, 0xb8, 0, 0, 0], 0x81), // operand-size prefix
        (&[0x67, 0x48, 0x81, 0xec, 0xb8, 0, 0, 0], 0x81), // address-size prefix
        (&[0xf3, 0x48, 0x81, 0xec, 0xb8, 0, 0, 0], 0x81), // REP prefix
        (&[0x2e, 0x48, 0x81, 0xec, 0xb8, 0, 0, 0], 0x81), // segment prefix
        (&[0x4c, 0x81, 0xec, 0xb8, 0, 0, 0], 0x81), // different REX
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_entry_sub_rsp_imm32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x81, 0xec, 0xb8, 0, 0, 0];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x81 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_bx_cx_uses_word_operands_and_preserves_registers() {
    let code = [0x66, 0x39, 0xcb]; // CMP BX,CX
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0001;
    cpu.gpr[Register::Rcx as usize] = 0x1122_3344_5566_0001;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.rip = RIP + code.len() as u64;
    // The low words compare equal even though their upper 48 bits differ.
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 6) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_cmp_bx_cx_flags_prove_bx_is_the_left_operand() {
    let code = [0x66, 0x39, 0xcb]; // CMP BX,CX computes BX - CX
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0000;
    cpu.gpr[Register::Rcx as usize] = 0x1122_3344_5566_0001;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.rip = RIP + code.len() as u64;
    // 16-bit 0 - 1 borrows and yields 0xffff: CF, PF, AF and SF are set.
    expected.rflags = (expected.rflags & !0x8d5) | 0x95;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_cmp_bx_cx_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let code = [0x66, 0x39];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_bx_cx_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x66, 0x39, 0xc1], 0x39),       // CMP CX,AX
        (&[0x66, 0x39, 0xd9], 0x39),       // CMP CX,BX (reverse operand roles)
        (&[0x66, 0x3b, 0xcb], 0x3b),       // CMP CX,BX (reverse opcode direction)
        (&[0x66, 0x01, 0xcb], 0x01),       // ADD BX,CX
        (&[0x39, 0xcb], 0x39),             // dword CMP EBX,ECX
        (&[0x48, 0x39, 0xcb], 0x39),       // qword CMP RBX,RCX
        (&[0x66, 0x67, 0x39, 0xcb], 0x39), // address-size prefix
        (&[0xf3, 0x66, 0x39, 0xcb], 0x39), // REP prefix
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_bx_cx_is_unsupported_in_cpl3_profile() {
    let bytes = [0x66, 0x39, 0xcb];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x66 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_entry_cmp_rdi_imm32_checks_handoff_gpa_without_writing_registers() {
    let code = [0x48, 0x81, 0xff, 0, 0, 0x10, 0]; // CMP RDI,0x00100000
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rdi as usize] = 0x0010_0000;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED | 0x8d5);
    let mut expected = before;
    expected.rip = RIP + code.len() as u64;
    // Equal operands set ZF; other arithmetic flags clear, all other bits stay.
    expected.rflags = (expected.rflags & !0x8d5) | (1 << 6) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_cmp_rdi_imm32_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffa..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x81, 0xff, 0, 0, 0x10]);
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
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_entry_cmp_rdi_imm32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x81, 0xff, 1, 0, 0x10, 0], 0x81), // different immediate
        (&[0x48, 0x81, 0xff, 0, 0, 0x20, 0], 0x81), // different immediate
        (&[0x48, 0x81, 0xf7, 0, 0, 0x10, 0], 0x81), // CMP RDI, different extension
        (&[0x48, 0x81, 0xef, 0, 0, 0x10, 0], 0x81), // SUB RDI,imm32
        (&[0x48, 0x81, 0xfb, 0, 0, 0x10, 0], 0x81), // CMP RBX,imm32
        (&[0x81, 0xff, 0, 0, 0x10, 0], 0x81),       // dword CMP EDI,imm32
        (&[0x48, 0x83, 0xff, 0], 0x83),             // CMP RDI,imm8
        (&[0x48, 0x39, 0xf8], 0x39),                // CMP RAX,RDI
        (&[0x48, 0x81, 0x3f, 0, 0, 0x10, 0], 0x81), // memory destination
        (&[0x66, 0x48, 0x81, 0xff, 0, 0, 0x10, 0], 0x81), // operand-size prefix
        (&[0x67, 0x48, 0x81, 0xff, 0, 0, 0x10, 0], 0x81), // address-size prefix
        (&[0xf3, 0x48, 0x81, 0xff, 0, 0, 0x10, 0], 0x81), // REP prefix
        (&[0x4c, 0x81, 0xff, 0, 0, 0x10, 0], 0x81), // different REX
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_entry_cmp_rdi_imm32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x81, 0xff, 0, 0, 0x10, 0];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x81 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_di_imm8_compares_only_low_word_and_updates_flags() {
    let code = [0x66, 0x83, 0xff, 0x01]; // CMP DI,1 (imm8 sign-extended to word)
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0001;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    let arithmetic_flags = 0x8d5;
    assert_eq!(result.rflags_defined, DEFINED | arithmetic_flags);
    let mut expected = before;
    expected.rip = RIP + code.len() as u64;
    // 16-bit 1 - 1 sets ZF and even parity; other arithmetic flags clear.
    expected.rflags = (expected.rflags & !arithmetic_flags) | (1 << 6) | (1 << 2);
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_cmp_di_imm8_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x66, 0x83, 0xff];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_cmp_di_imm8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x66, 0x83, 0xff, 0x02], 0x83),       // different immediate
        (&[0x66, 0x83, 0xff, 0xff], 0x83),       // sign-extended -1 immediate
        (&[0x66, 0x83, 0xfe, 0x01], 0x83),       // CMP SI,1
        (&[0x66, 0x83, 0xef, 0x01], 0x83),       // SUB DI,1
        (&[0x83, 0xff, 0x01], 0x83),             // dword CMP EDI,1
        (&[0x48, 0x83, 0xff, 0x01], 0x83),       // qword CMP RDI,1
        (&[0x66, 0x67, 0x83, 0xff, 0x01], 0x83), // address-size prefix
        (&[0xf3, 0x66, 0x83, 0xff, 0x01], 0x83), // REP prefix
        (&[0x66, 0x83, 0x3e, 0x01], 0x83),       // memory destination
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_di_imm8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x66, 0x83, 0xff, 0x01];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x66 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_bx_imm8_compares_only_low_word_and_updates_flags() {
    let code = [0x66, 0x83, 0xfb, 0x03]; // CMP BX,3 (imm8 sign-extended to word)
    let arithmetic_flags = 0x8d5;
    for (bx, expected_flags) in [(3, 0x44), (2, 0x95), (4, 0x00)] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0000 | bx;
        cpu.rflags = 0x202;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step(&mut cpu, &mut memory, &mut pio, ring0());

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, DEFINED | arithmetic_flags);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.rflags = (expected.rflags & !arithmetic_flags) | expected_flags;
        assert_eq!(cpu, expected, "BX={bx}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_bx_imm8_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x66, 0x83, 0xfb];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_cmp_bx_imm8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x66, 0x83, 0xfb, 0x02], 0x83),       // different immediate
        (&[0x66, 0x83, 0xfb, 0xff], 0x83),       // sign-extended -1 immediate
        (&[0x66, 0x83, 0xff, 0x03], 0x83),       // CMP DI,3
        (&[0x66, 0x83, 0xeb, 0x03], 0x83),       // SUB BX,3
        (&[0x83, 0xfb, 0x03], 0x83),             // dword CMP EBX,3
        (&[0x48, 0x83, 0xfb, 0x03], 0x83),       // qword CMP RBX,3
        (&[0x66, 0x67, 0x83, 0xfb, 0x03], 0x83), // address-size prefix
        (&[0xf3, 0x66, 0x83, 0xfb, 0x03], 0x83), // REP prefix
        (&[0x66, 0x83, 0x3e, 0x03], 0x83),       // memory destination
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_bx_imm8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x66, 0x83, 0xfb, 0x03];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x66 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_movzx_edi_handoff_field_zero_extends_and_preserves_flags() {
    let code = [0x0f, 0xb7, 0x7e, 0x08]; // MOVZX EDI,word ptr [RSI+8]
    let rip = 0x200d47;
    let mut memory = memory_with_code_at(&code, rip);
    memory.bytes[DATA_PHYSICAL + 8..DATA_PHYSICAL + 10].copy_from_slice(&[0x34, 0x12]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rdi as usize] = 0x1234;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA + 8, 2)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_edi_injected_read_fault_preserves_state() {
    let code = [0x0f, 0xb7, 0x7e, 0x08];
    let rip = 0x200d47;
    let mut memory = memory_with_code_at(&code, rip);
    let fault_address = DATA + 8;
    memory.fail_read_at = Some((fault_address, page_fault(0, fault_address)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(fault_address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(fault_address, 2)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_edi_noncanonical_address_faults_before_read() {
    let code = [0x0f, 0xb7, 0x7e, 0x08];
    let rip = 0x200d47;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsi as usize] = 0x0000_8000_0000_0000;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    match result.exit {
        PagedProbeExit::Fault(fault) => assert_eq!(fault.vector, 13),
        other => panic!("expected #GP for noncanonical RSI+8, got {other:?}"),
    }
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.read_addresses.is_empty());
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_edi_fetch_fault_does_not_commit() {
    let rip = RIP + 0xffc;
    let mut memory = memory_with_code_at(&[0x0f, 0xb7, 0x7e, 0x08], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_edi_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0xb7, 0x7e, 0x07], 0xb7),       // different displacement
        (&[0x0f, 0xb7, 0x7e, 0x09], 0xb7),       // different displacement
        (&[0x0f, 0xb7, 0x76, 0x08], 0xb7),       // ESI destination
        (&[0x0f, 0xb7, 0x7f, 0x08], 0xb7),       // RDI base
        (&[0x0f, 0xb7, 0x3e], 0xb7),             // no displacement
        (&[0x48, 0x0f, 0xb7, 0x7e, 0x08], 0xb7), // qword destination
        (&[0x66, 0x0f, 0xb7, 0x7e, 0x08], 0xb7), // operand-size prefix
        (&[0x67, 0x0f, 0xb7, 0x7e, 0x08], 0xb7), // address-size prefix
        (&[0xf3, 0x0f, 0xb7, 0x7e, 0x08], 0xb7), // REP prefix
        (&[0x0f, 0xb6, 0x7e, 0x08], 0xb6),       // byte source
        (&[0x0f, 0xbf, 0x7e, 0x08], 0xbf),       // sign extension
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
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_movzx_edi_is_unsupported_in_cpl3_profile() {
    let code = [0x0f, 0xb7, 0x7e, 0x08];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_movzx_ebx_second_handoff_field_zero_extends_and_preserves_flags() {
    let code = [0x0f, 0xb7, 0x5e, 0x0a]; // MOVZX EBX,word ptr [RSI+0xA]
    let rip = 0x200d4b;
    let mut memory = memory_with_code_at(&code, rip);
    memory.bytes[DATA_PHYSICAL + 0x0a..DATA_PHYSICAL + 0x0c].copy_from_slice(&[0x68, 0x24]);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before.clone();
    expected.gpr[Register::Rbx as usize] = 0x2468;
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA + 0x0a, 2)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_ebx_second_field_injected_read_fault_preserves_state() {
    let code = [0x0f, 0xb7, 0x5e, 0x0a];
    let rip = 0x200d4b;
    let mut memory = memory_with_code_at(&code, rip);
    let fault_address = DATA + 0x0a;
    memory.fail_read_at = Some((fault_address, page_fault(0, fault_address)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(fault_address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(fault_address, 2)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_ebx_second_field_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0xb7, 0x5e, 0x09], 0xb7),       // different displacement
        (&[0x0f, 0xb7, 0x5e, 0x0b], 0xb7),       // different displacement
        (&[0x0f, 0xb7, 0x56, 0x0a], 0xb7),       // EDX destination
        (&[0x0f, 0xb7, 0x5f, 0x0a], 0xb7),       // RDI base
        (&[0x0f, 0xb7, 0x1e], 0xb7),             // no displacement
        (&[0x48, 0x0f, 0xb7, 0x5e, 0x0a], 0xb7), // qword destination
        (&[0x66, 0x0f, 0xb7, 0x5e, 0x0a], 0xb7), // operand-size prefix
        (&[0x67, 0x0f, 0xb7, 0x5e, 0x0a], 0xb7), // address-size prefix
        (&[0xf3, 0x0f, 0xb7, 0x5e, 0x0a], 0xb7), // REP prefix
        (&[0x0f, 0xb6, 0x5e, 0x0a], 0xb6),       // byte source
        (&[0x0f, 0xbf, 0x5e, 0x0a], 0xbf),       // sign extension
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
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_movzx_ebx_second_field_is_unsupported_in_cpl3_profile() {
    let code = [0x0f, 0xb7, 0x5e, 0x0a];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_movzx_r14d_rsi_r11_byte_zero_extends_and_preserves_flags() {
    let code = [0x46, 0x0f, 0xb6, 0x34, 0x1e]; // MOVZX R14D,byte ptr [RSI+R11]
    let rip = 0x200e00;
    let mut memory = memory_with_code_at(&code, rip);
    memory.bytes[DATA_PHYSICAL + 0x18] = 0xa7;
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::R11 as usize] = 0x18;
    cpu.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000; // must not be used as the base
    cpu.gpr[Register::R14 as usize] = 0xffff_ffff_dead_beef;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = DEFINED & !(1 << 6);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, initial_defined);
    let mut expected = before;
    expected.gpr[Register::R14 as usize] = 0xa7; // R14D write zero-extends into R14.
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA + 0x18, 1)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r14d_rsi_r11_injected_read_fault_preserves_state() {
    let code = [0x46, 0x0f, 0xb6, 0x34, 0x1e];
    let rip = 0x200e00;
    let mut memory = memory_with_code_at(&code, rip);
    let fault_address = DATA + 0x18;
    memory.fail_read_at = Some((fault_address, page_fault(0, fault_address)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::R11 as usize] = 0x18;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(fault_address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(fault_address, 1)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r14d_rsi_r11_noncanonical_address_faults_before_read() {
    let code = [0x46, 0x0f, 0xb6, 0x34, 0x1e];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr[Register::Rsi as usize] = 0x0000_8000_0000_0000;
    cpu.gpr[Register::R11 as usize] = 0;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    match result.exit {
        PagedProbeExit::Fault(fault) => assert_eq!(fault.vector, 13),
        other => panic!("expected #GP for noncanonical RSI+R11, got {other:?}"),
    }
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.read_addresses.is_empty());
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r14d_rsi_r11_fetch_fault_on_sib_is_atomic() {
    let code = [0x46, 0x0f, 0xb6, 0x34, 0x1e];
    let rip = RIP + 0xffc;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 4, page_fault(0x10, rip + 4)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 4),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r14d_rsi_r11_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x46, 0x0f, 0xb6, 0x3c, 0x1e], 0xb6), // MOVZX R15D,[RSI+R11]
        (&[0x46, 0x0f, 0xb6, 0x34, 0x16], 0xb6), // index R10
        (&[0x46, 0x0f, 0xb6, 0x34, 0x1f], 0xb6), // base RDI
        (&[0x46, 0x0f, 0xb6, 0x74, 0x1e, 0x08], 0xb6), // displacement
        (&[0x42, 0x0f, 0xb6, 0x34, 0x1e], 0xb6), // different REX
        (&[0x4e, 0x0f, 0xb6, 0x34, 0x1e], 0xb6), // qword destination
        (&[0x66, 0x46, 0x0f, 0xb6, 0x34, 0x1e], 0xb6), // operand-size prefix
        (&[0x67, 0x46, 0x0f, 0xb6, 0x34, 0x1e], 0xb6), // address-size prefix
        (&[0x46, 0x0f, 0xb7, 0x34, 0x1e], 0xb7), // word source
        (&[0x46, 0x0f, 0xbe, 0x34, 0x1e], 0xbe), // sign extension
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
fn rust_helper_movzx_r14d_rsi_r11_is_unsupported_in_cpl3_profile() {
    let code = [0x46, 0x0f, 0xb6, 0x34, 0x1e];
    let mut memory = Memory::new(&code);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x46 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_movzx_r15d_rsi_r11_plus_one_zero_extends_and_preserves_flags() {
    let code = [0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01]; // MOVZX R15D,byte ptr [RSI+R11+1]
    let rip = 0x200e1a;
    let mut memory = memory_with_code_at(&code, rip);
    memory.bytes[DATA_PHYSICAL + 0x19] = 0x5c;
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::R11 as usize] = 0x18;
    cpu.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000; // neither the base nor the index
    cpu.gpr[Register::R15 as usize] = 0xffff_ffff_dead_beef;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let initial_defined = DEFINED & !(1 << 6);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), initial_defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, initial_defined);
    let mut expected = before;
    expected.gpr[Register::R15 as usize] = 0x5c; // R15D write zero-extends into R15.
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA + 0x19, 1)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r15d_rsi_r11_plus_one_injected_read_fault_preserves_state() {
    let code = [0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01];
    let rip = 0x200e1a;
    let fault_address = DATA + 0x19;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_read_at = Some((fault_address, page_fault(0, fault_address)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::R11 as usize] = 0x18;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(fault_address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(fault_address, 1)]);
    assert_eq!(memory.writes, 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r15d_rsi_r11_plus_one_fetch_fault_is_atomic() {
    let rip = RIP + 0xffa;
    let code = [0x46, 0x0f, 0xb6, 0x7c, 0x1e];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 5, page_fault(0x10, rip + 5)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 5),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_movzx_r15d_rsi_r11_plus_one_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x46, 0x0f, 0xb6, 0x3c, 0x1e], 0xb6), // R15D with no disp8
        (&[0x46, 0x0f, 0xb6, 0x74, 0x1e, 0x01], 0xb6), // destination R14D
        (&[0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x02], 0xb6), // different positive disp8
        (&[0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0xff], 0xb6), // signed negative disp8
        (&[0x46, 0x0f, 0xb6, 0x7c, 0x16, 0x01], 0xb6), // REX.X extends index to R10
        (&[0x46, 0x0f, 0xb6, 0x7c, 0x1f, 0x01], 0xb6), // base RDI
        (&[0x42, 0x0f, 0xb6, 0x7c, 0x1e, 0x01], 0xb6), // different REX
        (&[0x4e, 0x0f, 0xb6, 0x7c, 0x1e, 0x01], 0xb6), // qword destination
        (&[0x66, 0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01], 0xb6), // operand-size prefix
        (&[0x67, 0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01], 0xb6), // address-size prefix
        (&[0x46, 0x0f, 0xb7, 0x7c, 0x1e, 0x01], 0xb7), // word source
        (&[0x46, 0x0f, 0xbe, 0x7c, 0x1e, 0x01], 0xbe), // sign extension
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_movzx_r15d_rsi_r11_plus_one_is_unsupported_in_cpl3_profile() {
    let code = [0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01];
    let mut memory = Memory::new(&code);
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x46 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_movzx_r9d_r9b_zero_extends_byte_alias_and_preserves_flags() {
    let code = [0x45, 0x0f, 0xb6, 0xc9]; // MOVZX R9D,R9B
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0xdead_beef_1234_56ab;
    cpu.rflags = 0x8d7;
    let defined = DEFINED & !(1 << 6);
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::R9 as usize] = 0xab;
    expected.rip = RIP + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_r9d_r9b_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x45, 0x0f, 0xb6];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_movzx_r9d_r9b_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0xb6, 0xc9], 0xb6),             // ECX,CL
        (&[0x41, 0x0f, 0xb6, 0xc9], 0xb6),       // ECX,R9B
        (&[0x44, 0x0f, 0xb6, 0xc9], 0xb6),       // R9D,CL
        (&[0x45, 0x0f, 0xb6, 0xc8], 0xb6),       // R9D,R8B
        (&[0x45, 0x0f, 0xb7, 0xc9], 0xb7),       // word source
        (&[0x45, 0x0f, 0xbe, 0xc9], 0xbe),       // sign extension
        (&[0x49, 0x0f, 0xb6, 0xc9], 0xb6),       // qword destination
        (&[0x66, 0x45, 0x0f, 0xb6, 0xc9], 0xb6), // operand-size prefix
        (&[0xf3, 0x45, 0x0f, 0xb6, 0xc9], 0xb6), // REP prefix
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_movzx_r9d_r9b_is_unsupported_in_cpl3_profile() {
    let bytes = [0x45, 0x0f, 0xb6, 0xc9];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x45 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_movzx_ecx_bx_zero_extends_and_preserves_flags() {
    let code = [0x0f, 0xb7, 0xcb]; // MOVZX ECX,BX
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rbx as usize] = 0xfeed_face_cafe_8001;
    cpu.gpr[Register::Rcx as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::Rcx as usize] = 0x8001;
    expected.rip = RIP + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_ecx_bx_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let mut memory = memory_with_code_at(&[0x0f, 0xb7], rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_movzx_ecx_bx_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x0f, 0xb7, 0xc3], 0xb7),       // MOVZX EAX,BX
        (&[0x0f, 0xb7, 0xc9], 0xb7),       // MOVZX ECX,CX
        (&[0x0f, 0xb7, 0xd9], 0xb7),       // MOVZX EBX,CX
        (&[0x48, 0x0f, 0xb7, 0xcb], 0xb7), // qword destination
        (&[0x66, 0x0f, 0xb7, 0xcb], 0xb7), // word destination
        (&[0x44, 0x0f, 0xb7, 0xcb], 0xb7), // REX.R changes destination
        (&[0x41, 0x0f, 0xb7, 0xcb], 0xb7), // REX.B changes source
        (&[0x0f, 0xbf, 0xcb], 0xbf),       // MOVSX ECX,BX
        (&[0x0f, 0xb6, 0xcb], 0xb6),       // byte source
        (&[0x0f, 0xb7, 0x0b], 0xb7),       // memory source
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
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_movzx_ecx_bx_is_unsupported_in_cpl3_profile() {
    let code = [0x0f, 0xb7, 0xcb];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr[Register::Rbx as usize] = 0x8001;
    let before = cpu.clone();

    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_rdx_imm32_checks_exact_size_and_updates_flags() {
    let code = [0x48, 0x81, 0xfa, 0x9f, 0, 0, 0]; // CMP RDX,0x9f
    for (rdx, arithmetic_flags) in [(0x9fu64, (1 << 6) | (1 << 2)), (0x1_0000_009f, 1 << 2)] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rdx as usize] = rdx;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !(1 << 6);

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | 0x8d5);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        // CMP is qword-wide: equality at 0x9f sets ZF/PF; the high-qword
        // value differs by 2^32 and sets only PF for the resulting low byte.
        expected.rflags = (expected.rflags & !0x8d5) | arithmetic_flags;
        assert_eq!(cpu, expected);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_rdx_imm32_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffa..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x81, 0xfa, 0x9f, 0, 0]);
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
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_cmp_rdx_imm32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x81, 0xfa, 0x9e, 0, 0, 0], 0x81), // different immediate
        (&[0x48, 0x81, 0xfa, 0xa0, 0, 0, 0], 0x81), // different immediate
        (&[0x48, 0x81, 0xfa, 0x9f, 0xff, 0xff, 0xff], 0x81), // sign-extended negative imm32
        (&[0x48, 0x81, 0xff, 0x9f, 0, 0, 0], 0x81), // CMP RDI,0x9f
        (&[0x48, 0x81, 0xea, 0x9f, 0, 0, 0], 0x81), // SUB RDX,0x9f
        (&[0x48, 0x81, 0xf2, 0x9f, 0, 0, 0], 0x81), // different group extension
        (&[0x81, 0xfa, 0x9f, 0, 0, 0], 0x81),       // dword CMP EDX,imm32
        (&[0x48, 0x83, 0xfa, 0x9f], 0x83),          // imm8 form
        (&[0x48, 0x39, 0xd0], 0x39),                // register CMP
        (&[0x48, 0x81, 0x3a, 0x9f, 0, 0, 0], 0x81), // memory destination
        (&[0x66, 0x48, 0x81, 0xfa, 0x9f, 0, 0, 0], 0x81), // operand-size prefix
        (&[0x67, 0x48, 0x81, 0xfa, 0x9f, 0, 0, 0], 0x81), // address-size prefix
        (&[0xf3, 0x48, 0x81, 0xfa, 0x9f, 0, 0, 0], 0x81), // REP prefix
        (&[0x4c, 0x81, 0xfa, 0x9f, 0, 0, 0], 0x81), // different REX
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_rdx_imm32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x81, 0xfa, 0x9f, 0, 0, 0];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x81 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_r15_imm8_compares_qword_and_updates_flags() {
    let code = [0x49, 0x83, 0xff, 0x14]; // CMP R15,0x14; imm8 is sign-extended
    let arithmetic_flags = 0x8d5;
    for (r15, expected_flags) in [
        (0x14u64, 0x44),
        (0x13, 0x95),
        (0x15, 0x00),
        (0x1_0000_0014, 0x04),
        (0x8000_0000_0000_0000, 0x810),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R15 as usize] = r15;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !arithmetic_flags;

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | arithmetic_flags);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.rflags = (expected.rflags & !arithmetic_flags) | expected_flags;
        assert_eq!(cpu, expected, "R15={r15:#018x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_r15_imm8_fetch_fault_is_atomic() {
    let rip = RIP + 0xffc;
    let code = [0x49, 0x83, 0xff];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_cmp_r15_imm8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x49, 0x83, 0xff, 0x13], 0x83), // different immediate
        (&[0x49, 0x83, 0xff, 0xec], 0x83), // sign-extended negative immediate
        (&[0x49, 0x83, 0xfe, 0x14], 0x83), // CMP R14,0x14
        (&[0x4d, 0x83, 0xff, 0x14], 0x83), // different REX
        (&[0x41, 0x83, 0xff, 0x14], 0x83), // dword CMP R15D,0x14
        (&[0x49, 0x83, 0xf7, 0x14], 0x83), // XOR R15,0x14
        (&[0x49, 0x83, 0x3f, 0x14], 0x83), // memory CMP
        (&[0x48, 0x83, 0xff, 0x14], 0x83), // CMP RDI,0x14
        (&[0x66, 0x49, 0x83, 0xff, 0x14], 0x83),
        (&[0xf3, 0x49, 0x83, 0xff, 0x14], 0x83),
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_r15_imm8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x49, 0x83, 0xff, 0x14];
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
fn rust_helper_cmovz_r14d_r10d_copies_only_when_zf_is_set() {
    let code = [0x45, 0x0f, 0x44, 0xf2]; // CMOVZ R14D,R10D
    let zf = 1 << 6;
    for (zf_set, zf_defined, taken) in [
        (true, true, true),
        (false, true, false),
        (true, false, true),
        (false, false, false),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R14 as usize] = 0xfeed_face_1234_5678;
        cpu.gpr[Register::R10 as usize] = 0x8765_4321_abcd_ef09;
        cpu.rflags = if zf_set { 0x8d7 } else { 0x897 };
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = if zf_defined {
            DEFINED | zf
        } else {
            DEFINED & !zf
        };

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        if taken {
            // Source is read as R10D; the R14D write zero-extends into R14.
            expected.gpr[Register::R14 as usize] = 0xabcd_ef09;
        } else {
            // A false dword CMOV preserves the old low dword and still
            // zero-extends the destination, matching shared CMOV semantics.
            expected.gpr[Register::R14 as usize] =
                expected.gpr[Register::R14 as usize] as u32 as u64;
        }
        assert_eq!(cpu, expected, "ZF={zf_set}, defined={zf_defined}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmovz_r15d_r10d_copies_only_when_zf_is_set() {
    let code = [0x45, 0x0f, 0x44, 0xfa]; // CMOVZ R15D,R10D
    let zf = 1 << 6;
    for (zf_set, zf_defined, taken) in [
        (true, true, true),
        (false, true, false),
        (true, false, true),
        (false, false, false),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R15 as usize] = 0xfeed_face_1234_5678;
        cpu.gpr[Register::R10 as usize] = 0x8765_4321_abcd_ef09;
        cpu.rflags = if zf_set { 0x8d7 } else { 0x897 };
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = if zf_defined {
            DEFINED | zf
        } else {
            DEFINED & !zf
        };

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.gpr[Register::R15 as usize] = if taken {
            0xabcd_ef09
        } else {
            // Shared dword CMOV semantics zero-extend even when ZF is clear.
            0x1234_5678
        };
        assert_eq!(cpu, expected, "ZF={zf_set}, defined={zf_defined}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmovz_r15d_r10d_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x45, 0x0f, 0x44];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.rflags = 0x897;
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_cmovz_r15d_r10d_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x45, 0x0f, 0x45, 0xfa], 0x45), // CMOVNZ R15D,R10D
        (&[0x45, 0x0f, 0x44, 0xfb], 0x44), // source R11D
        (&[0x45, 0x0f, 0x44, 0x3a], 0x44), // memory source
        (&[0x41, 0x0f, 0x44, 0xfa], 0x44), // REX.R missing
        (&[0x44, 0x0f, 0x44, 0xfa], 0x44), // REX.B missing
        (&[0x4d, 0x0f, 0x44, 0xfa], 0x44), // qword width
        (&[0x66, 0x45, 0x0f, 0x44, 0xfa], 0x44),
        (&[0xf3, 0x45, 0x0f, 0x44, 0xfa], 0x44),
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmovz_r15d_r10d_is_unsupported_in_cpl3_profile() {
    let bytes = [0x45, 0x0f, 0x44, 0xfa];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x45 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmovz_r14d_r10d_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x45, 0x0f, 0x44];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_cmovz_r14d_r10d_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x45, 0x0f, 0x45, 0xf2], 0x45), // CMOVNZ R14D,R10D
        (&[0x45, 0x0f, 0x44, 0xf3], 0x44), // source R11D
        (&[0x45, 0x0f, 0x44, 0x32], 0x44), // memory source
        (&[0x41, 0x0f, 0x44, 0xf2], 0x44), // REX.R missing, destination ESI
        (&[0x44, 0x0f, 0x44, 0xf2], 0x44), // REX.B missing, source EDX
        (&[0x4d, 0x0f, 0x44, 0xf2], 0x44), // qword width
        (&[0x66, 0x45, 0x0f, 0x44, 0xf2], 0x44),
        (&[0xf3, 0x45, 0x0f, 0x44, 0xf2], 0x44),
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmovz_r14d_r10d_is_unsupported_in_cpl3_profile() {
    let bytes = [0x45, 0x0f, 0x44, 0xf2];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x45 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_ecx_imm32_uses_dword_operand_and_updates_flags() {
    let code = [0x81, 0xf9, 0xa0, 0, 0, 0]; // CMP ECX,0xa0
    for (rcx, arithmetic_flags) in [
        (0xfeed_face_0000_00a0u64, (1 << 6) | (1 << 2)),
        (0xdead_beef_0000_00a1u64, 0),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rcx as usize] = rcx;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !((1 << 4) | (1 << 6));

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | 0x8d5);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.rflags = (expected.rflags & !0x8d5) | arithmetic_flags;
        assert_eq!(cpu, expected);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_ecx_imm32_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffb..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x81, 0xf9, 0xa0, 0, 0]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffb;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 5, page_fault(0x10, cpu.rip + 5)));

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
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_cmp_ecx_imm32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x81, 0xf9, 0xa1, 0, 0, 0], 0x81), // different immediate
        (&[0x81, 0xf8, 0xa0, 0, 0, 0], 0x81), // CMP EAX,imm32
        (&[0x81, 0xfa, 0xa0, 0, 0, 0], 0x81), // CMP EDX,imm32
        (&[0x81, 0xe9, 0xa0, 0, 0, 0], 0x81), // SUB ECX,imm32
        (&[0x81, 0xd1, 0xa0, 0, 0, 0], 0x81), // different group extension
        (&[0x48, 0x81, 0xf9, 0xa0, 0, 0, 0], 0x81), // qword, sign-extended imm32
        (&[0x48, 0x81, 0xf9, 0xa0, 0xff, 0xff, 0xff], 0x81), // negative sign-extended imm32
        (&[0x66, 0x81, 0xf9, 0xa0, 0, 0], 0x81), // word operand
        (&[0x67, 0x81, 0xf9, 0xa0, 0, 0, 0], 0x81), // address-size prefix
        (&[0xf3, 0x81, 0xf9, 0xa0, 0, 0, 0], 0x81), // REP prefix
        (&[0x83, 0xf9, 0xa0], 0x83),          // sign-extended imm8 form
        (&[0x81, 0x3e, 0xa0, 0, 0, 0], 0x81), // memory destination
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_ecx_imm32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x81, 0xf9, 0xa0, 0, 0, 0];
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
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x81 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_cmp_edi_imm32_ignores_upper_rdi_and_updates_flags() {
    let code = [0x81, 0xff, 0x61, 0xff, 0, 0]; // CMP EDI,0xff61
    for (rdi, arithmetic_flags) in [
        (0xfeed_face_0000_ff61u64, (1 << 6) | (1 << 2)),
        (
            0xdead_beef_0000_ff60u64,
            (1 << 7) | (1 << 4) | (1 << 2) | (1 << 0),
        ),
        (0x1122_3344_0000_ff62u64, 0),
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rdi as usize] = rdi;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();
        let defined = DEFINED & !((1 << 4) | (1 << 6));

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined | 0x8d5);
        let mut expected = before;
        expected.rip = RIP + code.len() as u64;
        expected.rflags = (expected.rflags & !0x8d5) | arithmetic_flags;
        assert_eq!(cpu, expected, "RDI={rdi:#018x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_edi_imm32_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffb..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x81, 0xff, 0x61, 0xff, 0]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffb;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 5, page_fault(0x10, cpu.rip + 5)));

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
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_cmp_edi_imm32_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x81, 0xff, 0x60, 0xff, 0, 0], 0x81), // different immediate
        (&[0x81, 0xf8, 0x61, 0xff, 0, 0], 0x81), // CMP EAX,imm32
        (&[0x81, 0xf9, 0x61, 0xff, 0, 0], 0x81), // CMP ECX,imm32
        (&[0x81, 0xef, 0x61, 0xff, 0, 0], 0x81), // SUB EDI,imm32
        (&[0x81, 0xd7, 0x61, 0xff, 0, 0], 0x81), // different group extension
        (&[0x48, 0x81, 0xff, 0x61, 0xff, 0, 0], 0x81), // qword operand
        (&[0x66, 0x81, 0xff, 0x61, 0xff], 0x81), // word operand
        (&[0x67, 0x81, 0xff, 0x61, 0xff, 0, 0], 0x81), // address-size prefix
        (&[0xf3, 0x81, 0xff, 0x61, 0xff, 0, 0], 0x81), // REP prefix
        (&[0x83, 0xff, 0x61], 0x83),             // sign-extended imm8 form
        (&[0x81, 0x3e, 0x61, 0xff, 0, 0], 0x81), // memory destination
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_cmp_edi_imm32_is_unsupported_in_cpl3_profile() {
    let bytes = [0x81, 0xff, 0x61, 0xff, 0, 0];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x81 });
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
fn firmware_entry_lea_rcx_rip_disp32_sets_exact_target_only() {
    let code = [0x48, 0x8d, 0x0d, 0x02, 0x2b, 0, 0];
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.rip = RIP;
    cpu.gpr[Register::Rcx as usize] = 0x1122_3344_5566_7788;
    cpu.rflags = 0xd5a;
    let before = cpu.clone();
    let mut pio = Pio::default();
    let defined = DEFINED & !(1 << 2);

    let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    let mut expected = before;
    expected.gpr[Register::Rcx as usize] = RIP + code.len() as u64 + 0x2b02;
    expected.rip += code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn firmware_entry_lea_rcx_fetch_fault_does_not_commit() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffa..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x48, 0x8d, 0x0d, 0x02, 0x2b, 0]);
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
fn firmware_entry_lea_rcx_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x8d, 0x0d, 0x01, 0x2b, 0, 0], 0x8d), // displacement differs
        (&[0x48, 0x8d, 0x15, 0x02, 0x2b, 0, 0], 0x8d), // RDX destination
        (&[0x48, 0x8d, 0x3d, 0x02, 0x2b, 0, 0], 0x8d), // RDI destination
        (&[0x40, 0x8d, 0x0d, 0x02, 0x2b, 0, 0], 0x8d), // no REX.W
        (&[0x48, 0x8d, 0x0f], 0x8d),                   // register base, not RIP-relative
        (&[0x67, 0x48, 0x8d, 0x0d, 0x02, 0x2b, 0, 0], 0x8d), // address override
        (&[0x66, 0x48, 0x8d, 0x0d, 0x02, 0x2b, 0, 0], 0x8d), // extra prefix
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
fn firmware_entry_lea_rcx_is_unsupported_in_cpl3_profile() {
    let bytes = [0x48, 0x8d, 0x0d, 0x02, 0x2b, 0, 0];
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
fn rust_entry_lea_rdi_rsp_disp8_sets_effective_address_only() {
    let defined = DEFINED & !(1 << 6);
    for (displacement, code) in [
        (8u64, [0x48, 0x8d, 0x7c, 0x24, 0x08]),
        (0x50u64, [0x48, 0x8d, 0x7c, 0x24, 0x50]),
    ] {
        for rsp in [0x14_ffa0, 0x0000_8000_0000_0000] {
            let mut memory = Memory::new(&code);
            let mut cpu = state();
            cpu.gpr = gpr_patterns();
            cpu.gpr[Register::Rsp as usize] = rsp;
            cpu.gpr[Register::Rdi as usize] = 0x1122_3344_5566_7788;
            cpu.rflags = 0xd5a;
            let before = cpu.clone();
            let mut pio = Pio::default();

            let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

            assert_eq!(result.exit, PagedProbeExit::Retired);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, defined);
            let mut expected = before;
            expected.gpr[Register::Rdi as usize] = rsp.wrapping_add(displacement);
            expected.rip = RIP + code.len() as u64;
            assert_eq!(cpu, expected, "disp={displacement:#x}, rsp={rsp:#x}");
            assert_eq!((memory.reads, memory.writes), (0, 0));
            assert!(memory.write_addresses.is_empty());
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn rust_entry_lea_rdi_rsp_disp8_fetch_fault_is_atomic() {
    for code in [
        [0x48, 0x8d, 0x7c, 0x24, 0x08],
        [0x48, 0x8d, 0x7c, 0x24, 0x50],
    ] {
        let mut memory = Memory::new(&[]);
        memory.bytes[CODE_PHYSICAL + 0xffc..CODE_PHYSICAL + 0x1000].copy_from_slice(&code[..4]);
        let mut cpu = state();
        cpu.rip = RIP + 0xffc;
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let mut pio = Pio::default();
        memory.fail_fetch_at = Some((cpu.rip + 4, page_fault(0x10, cpu.rip + 4)));

        let result = step(&mut cpu, &mut memory, &mut pio, ring0());

        assert_eq!(
            result.exit,
            PagedProbeExit::Fault(od_isa::ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(RIP + 0x1000),
            }),
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_entry_lea_rdi_rsp_disp8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x8d, 0x7c, 0x24, 0x07], 0x8d), // different displacement
        (&[0x48, 0x8d, 0x7c, 0x24, 0x09], 0x8d), // different displacement
        (&[0x48, 0x8d, 0x7c, 0x24, 0x4f], 0x8d), // different displacement
        (&[0x48, 0x8d, 0x7c, 0x24, 0x51], 0x8d), // different displacement
        (&[0x48, 0x8d, 0x74, 0x24, 0x08], 0x8d), // RSI destination
        (&[0x4c, 0x8d, 0x7c, 0x24, 0x08], 0x8d), // REX.R destination
        (&[0x48, 0x8d, 0x7d, 0x08], 0x8d),       // RBP base
        (&[0x48, 0x8d, 0x7c, 0x04, 0x08], 0x8d), // indexed address
        (&[0x48, 0x8d, 0x7c, 0x64, 0x08], 0x8d), // non-exact SIB scale
        (&[0x48, 0x8d, 0x3c, 0x24], 0x8d),       // no displacement
        (&[0x48, 0x8d, 0xbc, 0x24, 0x08, 0, 0, 0], 0x8d), // disp32
        (&[0x67, 0x48, 0x8d, 0x7c, 0x24, 0x08], 0x8d), // address-size prefix
        (&[0x66, 0x48, 0x8d, 0x7c, 0x24, 0x08], 0x8d), // operand-size prefix
        (&[0xf3, 0x48, 0x8d, 0x7c, 0x24, 0x08], 0x8d), // REP prefix
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_entry_lea_rdi_rsp_disp8_is_unsupported_in_cpl3_profile() {
    for bytes in [
        [0x48, 0x8d, 0x7c, 0x24, 0x08],
        [0x48, 0x8d, 0x7c, 0x24, 0x50],
    ] {
        let mut memory = Memory::new(&bytes);
        let mut cpu = state();
        cpu.cr3 = Some(0x1000);
        cpu.gpr = gpr_patterns();
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
fn rust_helper_mov_ecx_dword_handoff_field_zero_extends_and_preserves_flags() {
    for (displacement, value, rip) in [
        (0x0c, 0x1234_5678u32, 0x200da3),
        (0x10, 0x89ab_cdefu32, 0x200db2),
    ] {
        let code = [0x8b, 0x4e, displacement as u8];
        let mut memory = memory_with_code_at(&code, rip);
        memory.bytes[DATA_PHYSICAL + displacement..DATA_PHYSICAL + displacement + 4]
            .copy_from_slice(&value.to_le_bytes());
        let mut cpu = state();
        cpu.rip = rip;
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsi as usize] = DATA;
        cpu.gpr[Register::Rcx as usize] = 0xaabb_ccdd_eeff_0011;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();

        let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

        assert_eq!(
            result.exit,
            PagedProbeExit::Retired,
            "disp={displacement:#x}"
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, DEFINED);
        let mut expected = before;
        expected.gpr[Register::Rcx as usize] = u64::from(value);
        expected.rip = rip + code.len() as u64;
        assert_eq!(cpu, expected, "disp={displacement:#x}");
        assert_eq!(memory.reads, 1);
        assert_eq!(memory.read_addresses, [(DATA + displacement as u64, 4)]);
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn rust_helper_mov_ecx_dword_handoff_field_read_fault_preserves_state() {
    for (displacement, rip) in [(0x0c, 0x200da3), (0x10, 0x200db2)] {
        let code = [0x8b, 0x4e, displacement as u8];
        let mut memory = memory_with_code_at(&code, rip);
        let fault_address = DATA + displacement;
        memory.fail_read_at = Some((fault_address, page_fault(0, fault_address)));
        let mut cpu = state();
        cpu.rip = rip;
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rsi as usize] = DATA;
        let before = cpu.clone();

        let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

        assert_eq!(
            result.exit,
            PagedProbeExit::Fault(od_isa::ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(fault_address),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!(memory.reads, 1);
        assert_eq!(memory.read_addresses, [(fault_address, 4)]);
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn rust_helper_mov_ecx_dword_handoff_field_neighbours_stay_unsupported() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x8b, 0x4e, 0x0b], 0x8b),       // different displacement
        (&[0x8b, 0x4e, 0x0d], 0x8b),       // between admitted fields
        (&[0x8b, 0x4e, 0x0f], 0x8b),       // before second admitted field
        (&[0x8b, 0x4e, 0x11], 0x8b),       // after second admitted field
        (&[0x8b, 0x56, 0x0c], 0x8b),       // EDX destination
        (&[0x8b, 0x56, 0x10], 0x8b),       // EDX destination
        (&[0x8b, 0x4f, 0x0c], 0x8b),       // RDI base
        (&[0x8b, 0x4f, 0x10], 0x8b),       // RDI base
        (&[0x8b, 0x0e], 0x8b),             // no displacement
        (&[0x48, 0x8b, 0x4e, 0x0c], 0x8b), // qword destination
        (&[0x66, 0x8b, 0x4e, 0x0c], 0x8b), // word destination
        (&[0x67, 0x8b, 0x4e, 0x0c], 0x8b), // address-size prefix
        (&[0xf3, 0x8b, 0x4e, 0x0c], 0x8b), // REP prefix
        (&[0x89, 0x4e, 0x0c], 0x89),       // store direction
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: *opcode },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "{bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
    }
}

#[test]
fn rust_helper_mov_ecx_dword_handoff_field_is_unsupported_in_cpl3_profile() {
    for displacement in [0x0c, 0x10] {
        let bytes = [0x8b, 0x4e, displacement];
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

        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x8b });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before);
        assert_eq!((memory.reads, memory.writes), (0, 0));
    }
}

#[test]
fn rust_helper_mov_edi_dword_handoff_field_zero_extends_and_preserves_flags() {
    let code = [0x8b, 0x7e, 0x14]; // MOV EDI,dword ptr [RSI+0x14]
    let rip = 0x200def;
    let value = 0x89ab_cdefu32;
    let mut memory = memory_with_code_at(&code, rip);
    memory.bytes[DATA_PHYSICAL + 0x14..DATA_PHYSICAL + 0x18].copy_from_slice(&value.to_le_bytes());
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    cpu.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::Rdi as usize] = u64::from(value);
    expected.rip = rip + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(DATA + 0x14, 4)]);
    assert_eq!(memory.writes, 0);
}

#[test]
fn rust_helper_mov_edi_dword_handoff_field_read_fault_preserves_state() {
    let code = [0x8b, 0x7e, 0x14];
    let rip = 0x200def;
    let fault_address = DATA + 0x14;
    let mut memory = memory_with_code_at(&code, rip);
    memory.fail_read_at = Some((fault_address, page_fault(0, fault_address)));
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rsi as usize] = DATA;
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(fault_address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_addresses, [(fault_address, 4)]);
    assert_eq!(memory.writes, 0);
}

#[test]
fn rust_helper_mov_edi_dword_handoff_field_noncanonical_address_faults_before_read() {
    let code = [0x8b, 0x7e, 0x14];
    let rip = 0x200def;
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr[Register::Rsi as usize] = 0x0000_8000_0000_0000;
    let before = cpu.clone();

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    match result.exit {
        PagedProbeExit::Fault(fault) => assert_eq!(fault.vector, 13),
        other => panic!("expected #GP for noncanonical RSI+0x14, got {other:?}"),
    }
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.read_addresses.is_empty());
    assert_eq!(memory.writes, 0);
}

#[test]
fn rust_helper_mov_edi_dword_handoff_field_neighbours_stay_unsupported() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x8b, 0x7e, 0x13], 0x8b),          // different disp8
        (&[0x8b, 0x7e, 0x15], 0x8b),          // adjacent disp8
        (&[0x8b, 0x76, 0x14], 0x8b),          // ESI destination
        (&[0x8b, 0x7f, 0x14], 0x8b),          // RDI base
        (&[0x8b, 0x3e], 0x8b),                // no displacement
        (&[0x8b, 0xbe, 0x14, 0, 0, 0], 0x8b), // disp32 form
        (&[0x48, 0x8b, 0x7e, 0x14], 0x8b),    // qword destination
        (&[0x66, 0x8b, 0x7e, 0x14], 0x8b),    // word destination
        (&[0x67, 0x8b, 0x7e, 0x14], 0x8b),    // address-size prefix
        (&[0xf3, 0x8b, 0x7e, 0x14], 0x8b),    // REP prefix
        (&[0x89, 0x7e, 0x14], 0x89),          // store direction
    ];
    for (bytes, opcode) in cases {
        let mut memory = Memory::new(bytes);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        let before = cpu.clone();
        let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

        assert_eq!(
            result.exit,
            PagedProbeExit::Unsupported { opcode: *opcode },
            "bytes {bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, DEFINED);
        assert_eq!(cpu, before, "bytes {bytes:02x?}");
        assert_eq!((memory.reads, memory.writes), (0, 0), "bytes {bytes:02x?}");
    }
}

#[test]
fn rust_helper_mov_edi_dword_handoff_field_is_unsupported_in_cpl3_profile() {
    let bytes = [0x8b, 0x7e, 0x14];
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

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x8b });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_ebp_ecx_dword_zero_extends_and_preserves_flags() {
    let code = [0x89, 0xcd]; // MOV EBP,ECX
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::Rcx as usize] = 0xfeed_face_89ab_cdef;
    cpu.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::Rbp as usize] = 0x89ab_cdef;
    expected.rip = RIP + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_ebp_ecx_fetch_fault_is_atomic() {
    let rip = RIP + 0xfff;
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xfff] = 0x89;
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 1, page_fault(0x10, rip + 1)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 1),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
}

#[test]
fn rust_helper_mov_ebp_ecx_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x89, 0xc5], 0x89),             // MOV EBP,EAX
        (&[0x89, 0xd5], 0x89),             // MOV EBP,EDX
        (&[0x89, 0xce], 0x89),             // MOV ESI,ECX
        (&[0x48, 0x89, 0xcd], 0x89),       // qword destination
        (&[0x66, 0x89, 0xcd], 0x89),       // word destination
        (&[0x8b, 0xcd], 0x8b),             // reverse direction
        (&[0x89, 0x0d, 0, 0, 0, 0], 0x89), // memory destination
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
fn rust_helper_mov_ebp_ecx_is_unsupported_in_cpl3_profile() {
    let bytes = [0x89, 0xcd];
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

    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x89 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_r12d_r9d_zero_extends_and_preserves_flags() {
    let code = [0x45, 0x89, 0xcc]; // MOV R12D,R9D
    let mut memory = Memory::new(&code);
    let mut cpu = state();
    cpu.gpr = gpr_patterns();
    cpu.gpr[Register::R9 as usize] = 0xfeed_face_89ab_cdef;
    cpu.gpr[Register::R12 as usize] = 0xaabb_ccdd_eeff_0011;
    cpu.rflags = 0x8d7;
    let before = cpu.clone();
    let mut pio = Pio::default();

    let result = step(&mut cpu, &mut memory, &mut pio, ring0());

    assert_eq!(result.exit, PagedProbeExit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, DEFINED);
    let mut expected = before;
    expected.gpr[Register::R12 as usize] = 0x89ab_cdef;
    expected.rip = RIP + code.len() as u64;
    assert_eq!(cpu, expected);
    assert_eq!((memory.reads, memory.writes), (0, 0));
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_mov_r12d_r9d_fetch_fault_is_atomic() {
    let rip = RIP + 0xffe;
    let code = [0x45, 0x89];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 2, page_fault(0x10, rip + 2)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 2),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_mov_r12d_r9d_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x45, 0x89, 0xc4], 0x89),       // MOV R12D,R8D
        (&[0x45, 0x89, 0xcd], 0x89),       // MOV R13D,R9D
        (&[0x41, 0x89, 0xcc], 0x89),       // MOV R12D,ECX
        (&[0x45, 0x8b, 0xe1], 0x8b),       // alternate opcode for same transfer
        (&[0x4d, 0x89, 0xcc], 0x89),       // qword destination
        (&[0x66, 0x45, 0x89, 0xcc], 0x89), // word destination
        (&[0x45, 0x89, 0x0c, 0x24], 0x89), // memory destination
        (&[0x44, 0x89, 0xcc], 0x89),       // REX.B omitted
        (&[0x40, 0x45, 0x89, 0xcc], 0x89), // redundant REX prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_mov_r12d_r9d_is_unsupported_in_cpl3_profile() {
    let bytes = [0x45, 0x89, 0xcc];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x45 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_shr_r12d_8_updates_result_flags_and_definedness() {
    let code = [0x41, 0xc1, 0xec, 0x08]; // SHR R12D,8
    let arithmetic_flags = 0x8d5;
    let defined = DEFINED | 0x10 | 0x800; // AF and OF start defined.
    for (input, result_value, result_flags) in [
        (0x8000_01ff, 0x0080_0001, 0x811),
        (0x8000_0080, 0x0080_0000, 0x815),
        (0x0000_0100, 0x0000_0001, 0x810),
        (0x0000_0000, 0x0000_0000, 0x854),
    ] {
        assert_eq!(input >> 8, result_value);
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::R12 as usize] = 0xaabb_ccdd_0000_0000 | input;
        cpu.gpr[Register::R9 as usize] = 0xfeed_face_89ab_cdef;
        cpu.rflags = 0x8d7;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            result.rflags_defined,
            (defined & !(0x10 | 0x800)) | (arithmetic_flags & !(0x10 | 0x800))
        );
        let mut expected = before;
        expected.gpr[Register::R12 as usize] = result_value;
        expected.rflags = (expected.rflags & !(arithmetic_flags & !0x10)) | result_flags;
        expected.rip = RIP + code.len() as u64;
        assert_eq!(cpu, expected, "input={input:#010x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_shr_r12d_8_fetch_fault_is_atomic() {
    let rip = RIP + 0xffd;
    let code = [0x41, 0xc1, 0xec];
    let mut memory = memory_with_code_at(&code, rip);
    let mut cpu = state();
    cpu.rip = rip;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    memory.fail_fetch_at = Some((rip + 3, page_fault(0x10, rip + 3)));

    let result = step(&mut cpu, &mut memory, &mut Pio::default(), ring0());

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(rip + 3),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_shr_r12d_8_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x41, 0xc1, 0xec, 0x07], 0xc1),       // SHR R12D,7
        (&[0x41, 0xc1, 0xec, 0x09], 0xc1),       // SHR R12D,9
        (&[0x41, 0xc1, 0xea, 0x08], 0xc1),       // SHR R10D,8
        (&[0x41, 0xc1, 0xfc, 0x08], 0xc1),       // SAR R12D,8
        (&[0x49, 0xc1, 0xec, 0x08], 0xc1),       // qword destination
        (&[0x41, 0xd1, 0xec], 0xd1),             // implicit count 1
        (&[0x41, 0xd3, 0xec], 0xd3),             // count from CL
        (&[0x41, 0xc1, 0x2c, 0x24, 0x08], 0xc1), // memory destination
        (&[0x66, 0x41, 0xc1, 0xec, 0x08], 0xc1), // word destination
        (&[0x40, 0x41, 0xc1, 0xec, 0x08], 0xc1), // redundant REX prefix
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
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_shr_r12d_8_is_unsupported_in_cpl3_profile() {
    let bytes = [0x41, 0xc1, 0xec, 0x08];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let result = execute::step_paged_cpl3_mov(
        &mut cpu,
        &mut memory,
        context(InstructionMode::Long64, PrivilegeLevel::Ring3),
        0xd00,
        DEFINED,
    );
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x41 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, DEFINED);
    assert_eq!(cpu, before);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}

#[test]
fn rust_helper_lea_edi_rcx_minus_a0_zero_extends_and_preserves_flags() {
    let code = [0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff];
    let defined = DEFINED & !(1 << 6);
    for rcx in [
        0xffff_ff12_3456_7890,
        0x40,
        0x0000_8000_0000_0100, // noncanonical 64-bit EA, truncated by EDI write
    ] {
        let mut memory = Memory::new(&code);
        let mut cpu = state();
        cpu.gpr = gpr_patterns();
        cpu.gpr[Register::Rcx as usize] = rcx;
        cpu.gpr[Register::Rdi as usize] = 0x1122_3344_5566_7788;
        cpu.rflags = 0xd5a;
        let before = cpu.clone();
        let mut pio = Pio::default();

        let result = step_with_defined(&mut cpu, &mut memory, &mut pio, ring0(), defined);

        assert_eq!(result.exit, PagedProbeExit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, defined);
        let mut expected = before;
        expected.gpr[Register::Rdi as usize] = rcx.wrapping_sub(0xa0) as u32 as u64;
        expected.rip = RIP + code.len() as u64;
        assert_eq!(cpu, expected, "RCX={rcx:#018x}");
        assert_eq!((memory.reads, memory.writes), (0, 0));
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_lea_edi_rcx_minus_a0_fetch_fault_is_atomic() {
    let mut memory = Memory::new(&[]);
    memory.bytes[CODE_PHYSICAL + 0xffb..CODE_PHYSICAL + 0x1000]
        .copy_from_slice(&[0x8d, 0xb9, 0x60, 0xff, 0xff]);
    let mut cpu = state();
    cpu.rip = RIP + 0xffb;
    cpu.gpr = gpr_patterns();
    let before = cpu.clone();
    let mut pio = Pio::default();
    memory.fail_fetch_at = Some((cpu.rip + 5, page_fault(0x10, cpu.rip + 5)));

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
    assert!(memory.write_addresses.is_empty());
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn rust_helper_lea_edi_rcx_minus_a0_neighbours_stay_unsupported_without_mutation() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x8d, 0xb9, 0x61, 0xff, 0xff, 0xff], 0x8d), // displacement -0x9f
        (&[0x8d, 0xb9, 0x5f, 0xff, 0xff, 0xff], 0x8d), // displacement -0xa1
        (&[0x8d, 0xb1, 0x60, 0xff, 0xff, 0xff], 0x8d), // ESI destination
        (&[0x8d, 0xba, 0x60, 0xff, 0xff, 0xff], 0x8d), // RDX base
        (&[0x8d, 0xbc, 0x01, 0x60, 0xff, 0xff, 0xff], 0x8d), // indexed source
        (&[0x8d, 0x79, 0x60], 0x8d),                   // disp8 form
        (&[0x48, 0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff], 0x8d), // qword destination
        (&[0x66, 0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff], 0x8d), // word destination
        (&[0x67, 0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff], 0x8d), // addr-size override
        (&[0x40, 0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff], 0x8d), // redundant REX
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
        assert_eq!((memory.reads, memory.writes), (0, 0), "{bytes:02x?}");
        assert!(memory.write_addresses.is_empty());
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn rust_helper_lea_edi_rcx_minus_a0_is_unsupported_in_cpl3_profile() {
    let bytes = [0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff];
    let mut memory = Memory::new(&bytes);
    let mut cpu = state();
    cpu.cr3 = Some(0x1000);
    cpu.gpr = gpr_patterns();
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
        (
            &[0x48, 0xbf, 0x4f, 0x44, 0x48, 0x41, 0x4e, 0x44, 0x4f, 0x46][..],
            0xbf,
        ),
        (
            &[0x49, 0xb8, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f][..],
            0xb8,
        ),
        (&[0x41, 0xb9, 0xff, 0xff, 0xff, 0xff][..], 0xb9),
        (&[0xbe, 0, 0, 0x10, 0][..], 0xbe),
        (&[0xba, 0, 0, 1, 0][..], 0xba),
        (&[0x48, 0x39, 0xc7][..], 0x39),
        (&[0x48, 0x39, 0xea][..], 0x39),
        (&[0x66, 0x39, 0xcb][..], 0x39),
        (&[0x75, 0x24][..], 0x75),
        (&[0x75, 0x2a][..], 0x75),
        (&[0x75, 0x38][..], 0x75),
        (&[0x75, 0x77][..], 0x75),
        (&[0x74, 0x3b][..], 0x74),
        (&[0x77, 0x0e][..], 0x77),
        (&[0x49, 0x89, 0xf8][..], 0x89),
        (&[0x48, 0x8d, 0x3d, 0x09, 0x2b, 0, 0][..], 0x8d),
        (&[0x48, 0x8d, 0x0d, 0x02, 0x2b, 0, 0][..], 0x8d),
        (&[0x48, 0x8d, 0x7c, 0x24, 0x08][..], 0x8d),
        (&[0x48, 0x8d, 0x7c, 0x24, 0x50][..], 0x8d),
        (&[0x48, 0x29, 0xf9][..], 0x29),
        (&[0x31, 0xc0][..], 0x31),
        (&[0x31, 0xc9][..], 0x31),
        (&[0x45, 0x31, 0xd2][..], 0x31),
        (&[0x45, 0x31, 0xf1][..], 0x31),
        (&[0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0][..], 0x33),
        (&[0x45, 0x31, 0xdb][..], 0x31),
        (&[0x66, 0x90][..], 0x90),
        (&[0x46, 0x0f, 0xb6, 0x34, 0x1e][..], 0xb6),
        (&[0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01][..], 0xb6),
        (&[0x45, 0x0f, 0xb6, 0xc9][..], 0xb6),
        (&[0x4d, 0x89, 0xdf][..], 0x89),
        (&[0x4d, 0x21, 0xc7][..], 0x21),
        (&[0x45, 0x89, 0xcc][..], 0x89),
        (&[0x41, 0xc1, 0xec, 0x08][..], 0xc1),
        (&[0x49, 0x83, 0xff, 0x14][..], 0x83),
        (&[0x45, 0x0f, 0x44, 0xf2][..], 0x44),
        (&[0x45, 0x0f, 0x44, 0xfa][..], 0x44),
        (&[0xf3, 0xaa][..], 0xaa),
        (&[0x4c, 0x89, 0xc7][..], 0x89),
        (&[0xe8, 0x83, 0x01, 0, 0][..], 0xe8),
        (&[0xe8, 0x8d, 0x02, 0, 0][..], 0xe8),
        (&[0xe8, 0x59, 0x08, 0, 0][..], 0xe8),
        (&[0x41, 0x56][..], 0x56),
        (&[0x41, 0x57][..], 0x57),
        (&[0x41, 0x55][..], 0x55),
        (&[0x41, 0x54][..], 0x54),
        (&[0x53][..], 0x53),
        (&[0x55][..], 0x55),
        (&[0x48, 0x83, 0xec, 0x58][..], 0x83),
        (&[0x48, 0x83, 0xec, 0x38][..], 0x83),
        (&[0x48, 0x81, 0xec, 0xb8, 0, 0, 0][..], 0x81),
        (&[0x0f, 0xb7, 0x7e, 0x08][..], 0xb7),
        (&[0x48, 0x89, 0xfb][..], 0x89),
        (&[0x48, 0x89, 0xf8][..], 0x89),
        (&[0x89, 0xcd][..], 0x89),
        (&[0x8d, 0xb9, 0x60, 0xff, 0xff, 0xff][..], 0x8d),
        (&[0x48, 0x81, 0xff, 0, 0, 0x10, 0][..], 0x81),
        (&[0x48, 0x81, 0xfa, 0x9f, 0, 0, 0][..], 0x81),
        (&[0x81, 0xf9, 0xa0, 0, 0, 0][..], 0x81),
        (&[0x81, 0xff, 0x61, 0xff, 0, 0][..], 0x81),
        (&[0x0f, 0x85, 0xd6, 0, 0, 0][..], 0x85),
        (&[0x0f, 0x84, 0xb4, 0, 0, 0][..], 0x84),
        (&[0x0f, 0xb7, 0xcb][..], 0xb7),
        (&[0x48, 0x39, 0x3e][..], 0x39),
        (&[0x0f, 0xb7, 0x5e, 0x0a][..], 0xb7),
        (&[0x8b, 0x4e, 0x10][..], 0x8b),
        (&[0x8b, 0x7e, 0x14][..], 0x8b),
        (&[0x66, 0x83, 0xff, 0x01][..], 0x83),
        (&[0x66, 0x83, 0xfb, 0x03][..], 0x83),
        (&[0x0f, 0x97, 0xc1][..], 0x97),
        (&[0x40, 0x0f, 0x93, 0xc7][..], 0x93),
        (&[0x41, 0x0f, 0x93, 0xc0][..], 0x93),
        (&[0x40, 0x0f, 0x92, 0xc7][..], 0x92),
        (&[0x40, 0x0f, 0x94, 0xc7][..], 0x94),
        (&[0x40, 0x08, 0xcf][..], 0x08),
        (&[0x41, 0x20, 0xf8][..], 0x20),
        (&[0x40, 0xf6, 0xc5, 0x07][..], 0xf6),
        (&[0x41, 0x84, 0xf8][..], 0x84),
        (&[0x8b, 0x4e, 0x0c][..], 0x8b),
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
