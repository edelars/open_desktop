use std::collections::HashMap;

use od_isa::execute::{
    self, Cr0Profile, Cr4Profile, DiagnosticCpl0PioU8V1, PagedCpl0PioU8V1, PioU8AdmissionError,
    PioU8Exit,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, Register,
    StiShadow,
};

const RIP: u64 = 0x4000;
const CR0: u64 = Cr0Profile::BASE;
const CR4: u64 = Cr4Profile::BASE;
const FLAGS_DEFINED: u64 = od_isa::INTEGER_FLAGS & !(1 << 4);

/// A small page-aware fixture: the selected root chooses a physical page for
/// the instruction's linear page. CR3 itself belongs to the memory backend,
/// not to od-isa's CPU state or this one-instruction entry.
#[derive(Clone)]
struct PagedMemory {
    physical: Vec<u8>,
    root: u64,
    translations: HashMap<u64, u64>,
    fetch_contexts: Vec<AccessContext>,
    fetch_error: Option<MemoryError>,
}

impl PagedMemory {
    fn new(root: u64, physical_page: u64, code: &[u8]) -> Self {
        let mut physical = vec![0; 0x8000];
        let start = physical_page as usize;
        physical[start..start + code.len()].copy_from_slice(code);
        let mut translations = HashMap::new();
        translations.insert(root, physical_page);
        Self {
            physical,
            root,
            translations,
            fetch_contexts: Vec::new(),
            fetch_error: None,
        }
    }

    fn fetch_count(&self) -> usize {
        self.fetch_contexts.len()
    }

    fn map(&self, address: u64) -> Option<usize> {
        let physical_page = *self.translations.get(&self.root)?;
        let page_offset = address.checked_sub(RIP)? as usize;
        Some(physical_page as usize + page_offset)
    }
}

impl GuestMemory for PagedMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetch_contexts.push(context);
        if let Some(error) = &self.fetch_error {
            return Err(error.clone());
        }
        let physical = self.map(address).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })?;
        self.physical
            .get(physical)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "unexpected read at {address:#x}"
        )))
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "unexpected write at {address:#x}"
        )))
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("PIO profile must not use atomic RAM operations")
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("PIO profile must not use atomic RAM operations")
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("PIO profile must not use atomic RAM operations")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!("PIO profile must not issue a memory fence")
    }
}

#[derive(Default)]
struct TestPio {
    read_value: u8,
    reads: Vec<u16>,
    writes: Vec<(u16, u8)>,
    fail_read: bool,
    fail_write: bool,
}

impl PortIo for TestPio {
    type Error = &'static str;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        self.reads.push(port);
        if self.fail_read {
            Err("read failed")
        } else {
            Ok(self.read_value)
        }
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        self.writes.push((port, value));
        if self.fail_write {
            Err("write failed")
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

fn initial_state() -> LongModeState {
    let mut state = LongModeState {
        cr3: None,
        rip: RIP,
        rflags: 0x8d7,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
    state.gpr[Register::Rdx as usize] = 0xabcd_1234_5678_9abc;
    state
}

fn run(
    state: &mut LongModeState,
    memory: &mut PagedMemory,
    pio: &mut TestPio,
    ctx: AccessContext,
) -> execute::PioU8Result<&'static str> {
    execute::step_paged_pio_u8(PagedCpl0PioU8V1, state, memory, pio, ctx, FLAGS_DEFINED)
}

#[test]
fn nonidentity_page_aware_fetch_executes_in_al_and_forces_explicit_origin() {
    for byte in [0x00, 0x80, 0xff] {
        let mut memory = PagedMemory::new(0x1000, 0x2000, &[0xec]);
        // The identity physical address contains a different, unsupported form.
        memory.physical[RIP as usize] = 0x90;
        let mut state = initial_state();
        let flags = state.rflags;
        let mut pio = TestPio {
            read_value: byte,
            ..TestPio::default()
        };

        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );

        assert_eq!(result.exit, PioU8Exit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state.rip, RIP + 1);
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0x1122_3344_5566_7700 | u64::from(byte)
        );
        assert_eq!(state.rflags, flags);
        assert_eq!(pio.reads, [0x9abc]);
        assert!(pio.writes.is_empty());
        assert_eq!(memory.fetch_count(), 1);
        assert_eq!(memory.fetch_contexts[0].origin, AccessOrigin::Explicit);
    }
}

#[test]
fn selected_page_root_controls_the_fetched_opcode() {
    let mut root_a = PagedMemory::new(0xa000, 0x2000, &[0xec]);
    root_a.translations.insert(0xb000, 0x3000);
    root_a.physical[0x3000] = 0xee;
    let mut root_b = root_a.clone();
    root_b.root = 0xb000;
    let mut first_state = initial_state();
    let mut first_pio = TestPio {
        read_value: 0x5a,
        ..TestPio::default()
    };
    assert_eq!(
        run(
            &mut first_state,
            &mut root_a,
            &mut first_pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        )
        .exit,
        PioU8Exit::Retired
    );
    assert_eq!(first_state.gpr[Register::Rax as usize] as u8, 0x5a);
    assert!(first_pio.writes.is_empty());

    let mut second_state = initial_state();
    let mut second_pio = TestPio::default();
    assert_eq!(
        run(
            &mut second_state,
            &mut root_b,
            &mut second_pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        )
        .exit,
        PioU8Exit::Retired
    );
    assert_eq!(second_pio.writes, [(0x9abc, 0x88)]);
    assert!(second_pio.reads.is_empty());
}

#[test]
fn not_present_and_nx_fetch_faults_are_preserved_without_pio_access() {
    let not_present = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x10),
        linear_address: Some(RIP),
    };
    let nx = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x11),
        linear_address: Some(RIP),
    };
    for fault in [not_present, nx] {
        let mut memory = PagedMemory::new(0x1000, 0x2000, &[0xec]);
        memory.fetch_error = Some(MemoryError::Architectural(fault.clone()));
        let mut state = initial_state();
        let before = state.clone();
        let mut pio = TestPio::default();

        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );

        assert_eq!(result.exit, PioU8Exit::Fault(fault));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state, before);
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());
    }
}

#[test]
fn backend_and_port_errors_leave_cpu_uncommitted() {
    let mut memory = PagedMemory::new(0x1000, 0x2000, &[0xec]);
    memory.fetch_error = Some(MemoryError::Backend("page backend failed".into()));
    let mut state = initial_state();
    let before = state.clone();
    let mut pio = TestPio::default();
    let result = run(
        &mut state,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
    );
    assert_eq!(
        result.exit,
        PioU8Exit::MemoryError(MemoryError::Backend("page backend failed".into()))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(pio.reads.is_empty());

    for (code, fail_read, fail_write) in [(&[0xec][..], true, false), (&[0xee][..], false, true)] {
        let mut memory = PagedMemory::new(0x1000, 0x2000, code);
        let mut state = initial_state();
        let before = state.clone();
        let mut pio = TestPio {
            fail_read,
            fail_write,
            ..TestPio::default()
        };
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert_eq!(
            result.exit,
            PioU8Exit::PortIoError(if fail_read {
                "read failed"
            } else {
                "write failed"
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state, before);
    }
}

#[test]
fn admission_rejections_and_unsupported_encodings_have_no_side_effects() {
    let mut cases = Vec::new();
    cases.push((
        initial_state(),
        context(InstructionMode::Long64, PrivilegeLevel::Ring1),
        PioU8AdmissionError::WrongPrivilege,
    ));
    let mut missing_cr = initial_state();
    missing_cr.cr0 = None;
    cases.push((
        missing_cr,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        PioU8AdmissionError::ControlRegistersMissing,
    ));
    let mut bad_cr0 = initial_state();
    bad_cr0.cr0 = Some(0);
    cases.push((
        bad_cr0,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        PioU8AdmissionError::ControlRegistersOutsideProfile,
    ));
    let mut bad_cr = initial_state();
    bad_cr.cr4 = Some(0);
    cases.push((
        bad_cr,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        PioU8AdmissionError::ControlRegistersOutsideProfile,
    ));
    let mut halted = initial_state();
    halted.activity = Some(CpuActivity::Halted);
    cases.push((
        halted,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        PioU8AdmissionError::Halted,
    ));
    let mut shadowed = initial_state();
    shadowed.sti_shadow = Some(StiShadow::Blocking);
    cases.push((
        shadowed,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        PioU8AdmissionError::StiShadowActive,
    ));

    for (mut state, context, expected) in cases {
        let mut memory = PagedMemory::new(0x1000, 0x2000, &[0xec]);
        let before = state.clone();
        let mut pio = TestPio::default();
        let result = run(&mut state, &mut memory, &mut pio, context);
        assert_eq!(result.exit, PioU8Exit::Rejected(expected));
        assert_eq!(state, before);
        assert_eq!(memory.fetch_count(), 0);
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());
    }

    for bytes in [
        &[0x66, 0xec][..],
        &[0xf3, 0xee][..],
        &[0x40, 0xec][..],
        &[0x90][..],
    ] {
        let mut memory = PagedMemory::new(0x1000, 0x2000, bytes);
        let mut state = initial_state();
        let before = state.clone();
        let mut pio = TestPio::default();
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        );
        assert!(matches!(result.exit, PioU8Exit::Unsupported { .. }));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(pio.reads.is_empty());
        assert!(pio.writes.is_empty());
    }
}

#[test]
fn admission_rejects_invalid_definedness_and_legacy_identity_entry_stays_closed() {
    let mut memory = PagedMemory::new(0x1000, 0x2000, &[0xec]);
    let mut state = initial_state();
    let before = state.clone();
    let mut pio = TestPio::default();
    let result = execute::step_paged_pio_u8(
        PagedCpl0PioU8V1,
        &mut state,
        &mut memory,
        &mut pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        u64::MAX,
    );
    assert_eq!(
        result.exit,
        PioU8Exit::Rejected(PioU8AdmissionError::InvalidRflagsDefined { value: u64::MAX })
    );
    assert_eq!(state, before);
    assert_eq!(memory.fetch_count(), 0);
    assert!(pio.reads.is_empty());

    let mut identity_memory = PagedMemory::new(0x1000, RIP, &[0xec]);
    let mut identity_state = initial_state();
    identity_state.cr0 = Some(CR0);
    identity_state.cr4 = Some(CR4);
    let mut identity_pio = TestPio::default();
    let legacy = execute::step_pio_u8(
        DiagnosticCpl0PioU8V1,
        &mut identity_state,
        &mut identity_memory,
        &mut identity_pio,
        context(InstructionMode::Long64, PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
    );
    assert_eq!(
        legacy.exit,
        PioU8Exit::Rejected(PioU8AdmissionError::ControlRegistersPresent)
    );
    assert_eq!(identity_memory.fetch_count(), 0);
    assert!(identity_pio.reads.is_empty());
}
