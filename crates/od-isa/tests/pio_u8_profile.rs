use od_isa::execute::{self, DiagnosticCpl0PioU8V1, PioU8AdmissionError, PioU8Exit, RunExit};
use od_isa::long_decode::{self, Operation};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, Register,
    StiShadow,
};

const RIP: u64 = 0x1000;
const FLAGS_DEFINED: u64 = od_isa::INTEGER_FLAGS & !(1 << 4);

#[derive(Clone)]
struct Memory {
    base: u64,
    bytes: Vec<u8>,
    fetch_contexts: Vec<AccessContext>,
    fetch_error: Option<MemoryError>,
}

impl Memory {
    fn code(bytes: &[u8]) -> Self {
        Self {
            base: RIP,
            bytes: bytes.to_vec(),
            fetch_contexts: Vec::new(),
            fetch_error: None,
        }
    }

    fn fetch_count(&self) -> usize {
        self.fetch_contexts.len()
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetch_contexts.push(context);
        if let Some(error) = &self.fetch_error {
            return Err(error.clone());
        }
        let offset = address.checked_sub(self.base).unwrap_or(u64::MAX) as usize;
        self.bytes
            .get(offset)
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

fn context(privilege: PrivilegeLevel, origin: AccessOrigin) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin,
    }
}

fn state_for(code: &[u8]) -> (LongModeState, Memory) {
    let mut state = LongModeState {
        rip: RIP,
        rflags: 0x8d7,
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
    state.gpr[Register::Rdx as usize] = 0xabcd_1234_5678_9abc;
    (state, Memory::code(code))
}

fn run(
    state: &mut LongModeState,
    memory: &mut Memory,
    pio: &mut TestPio,
    ctx: AccessContext,
) -> execute::PioU8Result<&'static str> {
    execute::step_pio_u8(
        DiagnosticCpl0PioU8V1,
        state,
        memory,
        pio,
        ctx,
        FLAGS_DEFINED,
    )
}

#[test]
fn decoder_models_only_the_exact_bare_pio_operations() {
    for (bytes, expected) in [
        (&[0xec][..], Operation::InAlDx),
        (&[0xee][..], Operation::OutDxAl),
    ] {
        let mut memory = Memory::code(bytes);
        let decoded = long_decode::decode(
            &mut memory,
            RIP,
            context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
        )
        .unwrap();
        assert_eq!(decoded.operation, expected);
        assert_eq!(decoded.length, 1);
    }
    for bytes in [
        &[0x66, 0xec][..],
        &[0xf3, 0xee][..],
        &[0x40, 0xec][..],
        &[0x90][..],
    ] {
        let mut memory = Memory::code(bytes);
        match long_decode::decode(
            &mut memory,
            RIP,
            context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
        ) {
            Ok(instruction) => assert!(!matches!(
                instruction.operation,
                Operation::InAlDx | Operation::OutDxAl
            )),
            Err(long_decode::DecodeError::Unsupported { .. }) => {}
            Err(error) => panic!("unexpected decode result: {error:?}"),
        }
    }
}

#[test]
fn in_al_dx_preserves_high_rax_and_commits_one_callback_and_retirement() {
    for byte in [0x00, 0x80, 0xff] {
        let (mut state, mut memory) = state_for(&[0xec]);
        let before_flags = state.rflags;
        let mut pio = TestPio {
            read_value: byte,
            ..TestPio::default()
        };
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(PrivilegeLevel::Ring0, AccessOrigin::DescriptorTable),
        );
        assert_eq!(result.exit, PioU8Exit::Retired);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state.rip, RIP + 1);
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0x1122_3344_5566_7700 | u64::from(byte)
        );
        assert_eq!(state.rflags, before_flags);
        assert_eq!(pio.reads, [0x9abc]);
        assert!(pio.writes.is_empty());
        assert_eq!(memory.fetch_count(), 1);
        assert_eq!(
            memory.fetch_contexts[0],
            context(PrivilegeLevel::Ring0, AccessOrigin::Explicit)
        );
    }
}

#[test]
fn out_dx_al_uses_low_sixteen_dx_and_only_low_byte_rax_once() {
    let (mut state, mut memory) = state_for(&[0xee]);
    let before = state.clone();
    let mut pio = TestPio::default();
    let result = run(
        &mut state,
        &mut memory,
        &mut pio,
        context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
    );
    assert_eq!(result.exit, PioU8Exit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(pio.writes, [(0x9abc, 0x88)]);
    assert!(pio.reads.is_empty());
    assert_eq!(state.rip, RIP + 1);
    assert_eq!(
        state.gpr[Register::Rax as usize],
        before.gpr[Register::Rax as usize]
    );
    assert_eq!(state.rflags, before.rflags);
}

#[test]
fn admission_rejections_precede_fetch_and_pio() {
    let cases = [
        (
            PrivilegeLevel::Ring1,
            None,
            None,
            None,
            None,
            PioU8AdmissionError::WrongPrivilege,
        ),
        (
            PrivilegeLevel::Ring2,
            None,
            None,
            None,
            None,
            PioU8AdmissionError::WrongPrivilege,
        ),
        (
            PrivilegeLevel::Ring3,
            None,
            None,
            None,
            None,
            PioU8AdmissionError::WrongPrivilege,
        ),
        (
            PrivilegeLevel::Ring0,
            Some(1),
            None,
            None,
            None,
            PioU8AdmissionError::ControlRegistersPresent,
        ),
        (
            PrivilegeLevel::Ring0,
            None,
            Some(1),
            None,
            None,
            PioU8AdmissionError::ControlRegistersPresent,
        ),
        (
            PrivilegeLevel::Ring0,
            None,
            None,
            Some(CpuActivity::Halted),
            None,
            PioU8AdmissionError::Halted,
        ),
        (
            PrivilegeLevel::Ring0,
            None,
            None,
            None,
            Some(StiShadow::Blocking),
            PioU8AdmissionError::StiShadowActive,
        ),
    ];
    for (privilege, cr0, cr4, activity, shadow, expected) in cases {
        let (mut state, mut memory) = state_for(&[0xec]);
        state.cr0 = cr0;
        state.cr4 = cr4;
        state.activity = activity;
        state.sti_shadow = shadow;
        let before = state.clone();
        let mut pio = TestPio::default();
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(privilege, AccessOrigin::Explicit),
        );
        assert_eq!(result.exit, PioU8Exit::Rejected(expected));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.fetch_count(), 0);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }

    let (mut state, mut memory) = state_for(&[0xec]);
    let before = state.clone();
    let mut pio = TestPio::default();
    let result = execute::step_pio_u8(
        DiagnosticCpl0PioU8V1,
        &mut state,
        &mut memory,
        &mut pio,
        context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
        u64::MAX,
    );
    assert_eq!(
        result.exit,
        PioU8Exit::Rejected(PioU8AdmissionError::InvalidRflagsDefined { value: u64::MAX })
    );
    assert_eq!(state, before);
    assert_eq!(memory.fetch_count(), 0);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn fetch_and_port_errors_are_distinct_and_leave_cpu_uncommitted() {
    for error in [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(RIP),
        }),
        MemoryError::Backend("fetch backend failed".into()),
    ] {
        let (mut state, mut memory) = state_for(&[0xec]);
        memory.fetch_error = Some(error.clone());
        let before = state.clone();
        let mut pio = TestPio::default();
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
        );
        match error {
            MemoryError::Architectural(fault) => assert_eq!(result.exit, PioU8Exit::Fault(fault)),
            other => assert_eq!(result.exit, PioU8Exit::MemoryError(other)),
        }
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }

    for (code, fail_read) in [(&[0xec][..], true), (&[0xee][..], false)] {
        let (mut state, mut memory) = state_for(code);
        let before = state.clone();
        let mut pio = TestPio {
            fail_read,
            fail_write: !fail_read,
            ..TestPio::default()
        };
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
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
        assert_eq!(pio.reads.len() + pio.writes.len(), 1);
    }
}

#[test]
fn invalid_instruction_fault_is_not_reported_as_backend_or_pio_error() {
    let (mut state, mut memory) = state_for(&[0x0f, 0x0b]);
    let before = state.clone();
    let mut pio = TestPio::default();
    let result = run(
        &mut state,
        &mut memory,
        &mut pio,
        context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
    );
    assert_eq!(
        result.exit,
        PioU8Exit::Fault(ArchitecturalFault {
            vector: 6,
            error_code: None,
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());
}

#[test]
fn unsupported_forms_have_no_pio_effect_and_legacy_entry_stays_closed() {
    for code in [&[0x66, 0xec][..], &[0xf3, 0xee][..], &[0x90][..]] {
        let (mut state, mut memory) = state_for(code);
        let before = state.clone();
        let mut pio = TestPio::default();
        let result = run(
            &mut state,
            &mut memory,
            &mut pio,
            context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
        );
        assert!(matches!(result.exit, PioU8Exit::Unsupported { .. }));
        assert_eq!(state, before);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }

    for code in [&[0xec][..], &[0xee][..]] {
        let (mut state, mut memory) = state_for(code);
        let before = state.clone();
        let result = execute::run(
            &mut state,
            &mut memory,
            od_isa::execute::RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(PrivilegeLevel::Ring0, AccessOrigin::Explicit),
            },
            || false,
        );
        assert!(matches!(result.exit, RunExit::Unsupported { .. }));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}
