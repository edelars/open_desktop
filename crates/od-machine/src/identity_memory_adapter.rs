//! Diagnostic flat identity bridge from `od-isa::GuestMemory` to production
//! `MachineBus`. Linear addresses are treated as physical addresses here.
//! This intentionally performs no paging, permission checks, or full Long64
//! machine integration; `page_privilege` is retained in the trace so callers
//! can verify the CPU contract context while this diagnostic mode is active.

use crate::{GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MemoryRegionKind};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError, PrivilegeLevel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticFlatAccess {
    pub kind: AccessKind,
    pub page_privilege: PrivilegeLevel,
}

/// Diagnostic flat identity adapter. Production memory and device ownership
/// remain in `MachineBus`; this adapter creates no parallel map or backing.
pub struct IdentityMemoryAdapter<'a> {
    bus: &'a mut MachineBus,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    last_access: Option<DiagnosticFlatAccess>,
}

impl<'a> IdentityMemoryAdapter<'a> {
    pub fn new(
        bus: &'a mut MachineBus,
        committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> Self {
        Self {
            bus,
            committed_write,
            last_access: None,
        }
    }

    pub const fn last_access(&self) -> Option<DiagnosticFlatAccess> {
        self.last_access
    }

    fn record(&mut self, kind: AccessKind, context: AccessContext) {
        self.last_access = Some(DiagnosticFlatAccess {
            kind,
            page_privilege: context.page_privilege(),
        });
    }

    fn error(error: impl std::fmt::Display) -> MemoryError {
        // Machine/backend failures are not architectural faults. In particular,
        // identity translation does not turn holes into #PF.
        MemoryError::Backend(error.to_string())
    }
}

impl GuestMemory for IdentityMemoryAdapter<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.record(AccessKind::Fetch, context);
        let physical = GuestPhysicalAddress::new(address);
        match self.bus.memory_kind(physical, 1).map_err(Self::error)? {
            MemoryRegionKind::Ram => {}
            MemoryRegionKind::Mmio => {
                return Err(Self::error("instruction fetch from MMIO is unsupported"));
            }
            kind => {
                return Err(Self::error(format!(
                    "instruction fetch from {kind:?} memory is unsupported"
                )));
            }
        }
        let mut byte = [0];
        self.bus
            .read_flat_bytes(physical, &mut byte)
            .map_err(Self::error)?;
        Ok(byte[0])
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.record(AccessKind::Read, context);
        self.bus
            .read_flat_bytes(GuestPhysicalAddress::new(address), destination)
            .map_err(Self::error)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.record(AccessKind::Write, context);
        self.bus
            .write_flat_bytes(
                GuestPhysicalAddress::new(address),
                source,
                &mut *self.committed_write,
            )
            .map_err(Self::error)
    }

    fn atomic_exchange_integer(
        &mut self,
        _address: u64,
        context: AccessContext,
        _size: od_isa::OperandSize,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.record(AccessKind::Write, context);
        Err(Self::error(
            "atomic memory operations are unsupported in diagnostic flat mode",
        ))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _address: u64,
        context: AccessContext,
        _size: od_isa::OperandSize,
        _addend: u64,
    ) -> Result<u64, MemoryError> {
        self.record(AccessKind::Write, context);
        Err(Self::error(
            "atomic memory operations are unsupported in diagnostic flat mode",
        ))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _address: u64,
        context: AccessContext,
        _size: od_isa::OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.record(AccessKind::Write, context);
        Err(Self::error(
            "atomic memory operations are unsupported in diagnostic flat mode",
        ))
    }

    fn atomic_rmw_integer(
        &mut self,
        _address: u64,
        context: AccessContext,
        _size: od_isa::OperandSize,
        _update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        // S6-20: the single LOCK/XCHG read-modify-write entry point is refused here too,
        // otherwise the trait default would run it over MachineBus (including MMIO).
        self.record(AccessKind::Write, context);
        Err(Self::error(
            "atomic memory operations are unsupported in diagnostic flat mode",
        ))
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.record(AccessKind::Read, context);
        Err(Self::error(
            "memory fences are unsupported in diagnostic flat mode",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusDevice, DeviceAccess, DeviceError, MachineMemoryMap, MemoryRegionSpec, RamLimits,
    };
    use od_isa::execute::{RunConfig, RunExit, run};
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
    use od_isa::{LongModeState, OperandSize, Register};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct DeviceState {
        calls: usize,
        value: u64,
    }

    struct CountingDevice(Arc<Mutex<DeviceState>>);

    impl BusDevice for CountingDevice {
        fn read(&mut self, access: crate::DeviceAccess) -> Result<u64, DeviceError> {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            if access.offset >= 8 {
                return Err(DeviceError::UnknownOffset);
            }
            Ok(state.value)
        }

        fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            if access.offset >= 8 {
                return Err(DeviceError::UnknownOffset);
            }
            state.value = value;
            Ok(())
        }
    }

    fn setup() -> (MachineBus, Arc<Mutex<DeviceState>>) {
        let state = Arc::new(Mutex::new(DeviceState::default()));
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(0x1000), 0x100).unwrap(),
                    MemoryRegionKind::Ram,
                ),
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(0x2000), 0x100).unwrap(),
                    MemoryRegionKind::Mmio,
                ),
            ],
        )
        .unwrap();
        let bus = MachineBus::new(
            map,
            RamLimits::new(0x100),
            vec![Box::new(CountingDevice(Arc::clone(&state)))],
            vec![crate::MmioBinding {
                region_start: GuestPhysicalAddress::new(0x2000),
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        (bus, state)
    }

    fn context(origin: AccessOrigin) -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring3,
            origin,
        }
    }

    #[test]
    fn diagnostic_flat_routes_ram_fetch_read_write_and_descriptor_context() {
        let (mut bus, _) = setup();
        let mut commits = Vec::new();
        let mut callback = |range| commits.push(range);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        memory
            .write(0x1000, context(AccessOrigin::Explicit), &[0x90, 0xc3])
            .unwrap();
        assert_eq!(
            memory.fetch(0x1000, context(AccessOrigin::Explicit)),
            Ok(0x90)
        );
        let mut data = [0; 2];
        memory
            .read(0x1000, context(AccessOrigin::DescriptorTable), &mut data)
            .unwrap();
        assert_eq!(data, [0x90, 0xc3]);
        assert_eq!(
            memory.last_access(),
            Some(DiagnosticFlatAccess {
                kind: AccessKind::Read,
                page_privilege: PrivilegeLevel::Ring0,
            })
        );
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].start(), GuestPhysicalAddress::new(0x1000));
    }

    #[test]
    fn failures_leave_read_destination_and_ram_write_callback_unchanged() {
        let (mut bus, _) = setup();
        let commits = Cell::new(0);
        let mut callback = |_| commits.set(commits.get() + 1);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        let mut destination = [0xa5; 4];
        let error = memory
            .read(0x10fe, context(AccessOrigin::Explicit), &mut destination)
            .unwrap_err();
        assert!(matches!(error, MemoryError::Backend(_)));
        assert_eq!(destination, [0xa5; 4]);
        assert!(
            memory
                .write(0x10fe, context(AccessOrigin::Explicit), &[1, 2, 3, 4])
                .is_err()
        );
        assert_eq!(commits.get(), 0);
    }

    #[test]
    fn mmio_width_is_one_device_operation_and_fetch_is_rejected() {
        let (mut bus, state) = setup();
        let commits = Cell::new(0);
        let mut callback = |_| commits.set(commits.get() + 1);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        memory
            .write(
                0x2000,
                context(AccessOrigin::Explicit),
                &[0x78, 0x56, 0x34, 0x12],
            )
            .unwrap();
        assert_eq!(state.lock().unwrap().calls, 1);
        let mut readback = [0; 4];
        memory
            .read(0x2000, context(AccessOrigin::Explicit), &mut readback)
            .unwrap();
        assert_eq!(state.lock().unwrap().calls, 2);
        assert_eq!(readback, [0x78, 0x56, 0x34, 0x12]);
        assert!(matches!(
            memory.fetch(0x2000, context(AccessOrigin::Explicit)),
            Err(MemoryError::Backend(_))
        ));
        assert_eq!(state.lock().unwrap().calls, 2);
        assert_eq!(commits.get(), 0);
    }

    #[test]
    fn device_error_has_no_effect_and_successful_mmio_write_has_no_ram_callback() {
        let (mut bus, state) = setup();
        let commits = Cell::new(0);
        let mut callback = |_| commits.set(commits.get() + 1);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        assert!(
            memory
                .write(0x2008, context(AccessOrigin::Explicit), &[1])
                .is_err()
        );
        assert_eq!(state.lock().unwrap().calls, 1);
        assert_eq!(commits.get(), 0);
        memory
            .write(0x1000, context(AccessOrigin::Explicit), &[0x5a])
            .unwrap();
        assert_eq!(commits.get(), 1);
        assert_eq!(state.lock().unwrap().calls, 1);
    }

    #[test]
    fn unsupported_atomic_and_fence_operations_fail_before_effects() {
        let (mut bus, _) = setup();
        let commits = Cell::new(0);
        let mut callback = |_| commits.set(commits.get() + 1);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        let ctx = context(AccessOrigin::Explicit);
        assert!(matches!(
            memory.atomic_exchange_integer(0x1000, ctx, OperandSize::Dword, 1),
            Err(MemoryError::Backend(_))
        ));
        assert!(matches!(
            memory.atomic_fetch_add_integer(0x1000, ctx, OperandSize::Dword, 1),
            Err(MemoryError::Backend(_))
        ));
        assert!(matches!(
            memory.atomic_compare_exchange_integer(0x1000, ctx, OperandSize::Dword, 0, 1),
            Err(MemoryError::Backend(_))
        ));
        let mut update_calls = 0;
        assert!(matches!(
            memory.atomic_rmw_integer(0x1000, ctx, OperandSize::Dword, &mut |old| {
                update_calls += 1;
                old.wrapping_add(1)
            }),
            Err(MemoryError::Backend(_))
        ));
        assert_eq!(update_calls, 0);
        assert!(matches!(memory.mfence(ctx), Err(MemoryError::Backend(_))));
        assert_eq!(commits.get(), 0);
    }

    #[test]
    fn lock_dword_forms_in_diagnostic_run_are_refused_without_effects() {
        const CODE: u64 = 0x1000;
        const DATA: u64 = 0x1080;
        let (mut bus, _) = setup();
        // LOCK INC dword [RBX]
        for (offset, byte) in [0xf0u8, 0xff, 0x03].into_iter().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(CODE + offset as u64),
                crate::AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
        }
        bus.write_physical(
            GuestPhysicalAddress::new(DATA),
            crate::AccessWidth::Dword,
            0x1122_3344,
            |_| {},
        )
        .unwrap();
        let mut state = LongModeState::default();
        state.rip = CODE;
        state.gpr[Register::Rbx as usize] = DATA;
        let before = state.clone();
        let commits = Cell::new(0);
        let mut callback = |_| commits.set(commits.get() + 1);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        let outcome = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert!(matches!(
            outcome.exit,
            RunExit::MemoryError(MemoryError::Backend(_))
        ));
        assert_eq!(outcome.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(commits.get(), 0);
        let mut bytes = [0u8; 4];
        let mut readback = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        readback
            .read(DATA, context(AccessOrigin::Explicit), &mut bytes)
            .unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 0x1122_3344);
    }

    #[test]
    fn interpreter_executes_through_production_bus_then_faults_without_retirement() {
        const CODE: u64 = 0x1000;
        const DATA: u64 = 0x1080;
        const UNMAPPED: u64 = 0x3000;
        let (mut bus, _) = setup();
        // MOV RAX, [RBX]; MOV [RBX], RCX; MOV RCX, [RDX]. The last operand
        // deliberately targets a hole after two successfully retired ops.
        let program: [u8; 9] = [0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b, 0x48, 0x8b, 0x0a];
        for (offset, byte) in program.into_iter().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(CODE + offset as u64),
                crate::AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
        }
        let original = 0x0123_4567_89ab_cdefu64;
        bus.write_physical(
            GuestPhysicalAddress::new(DATA),
            crate::AccessWidth::Qword,
            original,
            |_| {},
        )
        .unwrap();

        let mut state = LongModeState::default();
        state.rip = CODE;
        state.gpr[Register::Rbx as usize] = DATA;
        state.gpr[Register::Rdx as usize] = UNMAPPED;
        state.gpr[Register::Rcx as usize] = 0xfedc_ba98_7654_3210;
        let replacement = state.gpr[Register::Rcx as usize];
        let commits = std::cell::RefCell::new(Vec::new());
        let mut callback = |range| commits.borrow_mut().push(range);
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        let first = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(first.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(first.instructions_executed, 2);
        assert_eq!(state.rip, CODE + 6);
        assert_eq!(state.gpr[Register::Rax as usize], original);
        assert_eq!(state.gpr[Register::Rcx as usize], replacement);

        let cpu_before_fault = state.clone();
        let ram_before_fault = replacement.to_le_bytes();
        let second = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert!(matches!(
            second.exit,
            RunExit::MemoryError(od_isa::memory::MemoryError::Backend(_))
        ));
        assert_eq!(second.instructions_executed, 0);
        assert_eq!(state, cpu_before_fault);
        assert_eq!(state.rip, CODE + 6);
        let mut ram_after_fault = [0; 8];
        memory
            .bus
            .read_flat_bytes(GuestPhysicalAddress::new(DATA), &mut ram_after_fault)
            .unwrap();
        assert_eq!(ram_after_fault, ram_before_fault);
        let committed = commits.borrow();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].start(), GuestPhysicalAddress::new(DATA));
        assert_eq!(committed[0].len(), 8);
    }
}
