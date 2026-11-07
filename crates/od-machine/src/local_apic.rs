//! A deliberately incomplete one-CPU xAPIC register and fixed interrupt model.
//!
//! The version register is unavailable: its Max LVT field cannot represent
//! zero implemented LVT entries. No CPU interrupt delivery is performed.
use crate::apic_pair::PairState;
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use std::sync::{Arc, Mutex};

pub const LOCAL_APIC_MMIO_BASE: u64 = 0xfee0_0000;
pub const LOCAL_APIC_MMIO_SIZE: u64 = 0x1000;

#[derive(Clone, Debug)]
pub struct LocalApic(Arc<Mutex<PairState>>);

/// Control operations address the same state as the device on the bus.
#[derive(Clone, Debug)]
pub struct LocalApicHandle(pub(crate) Arc<Mutex<PairState>>);

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct State {
    pub(crate) global_enabled: bool,
    pub(crate) tpr: u8,
    pub(crate) svr: u16,
    pub(crate) icr_low: u32,
    pub(crate) icr_high: u32,
    pub(crate) irr: [u32; 8],
    pub(crate) isr: [u32; 8],
    pub(crate) tmr: [u32; 8],
}

impl Default for State {
    fn default() -> Self {
        Self {
            global_enabled: true,
            tpr: 0,
            svr: 0xff,
            icr_low: 0,
            icr_high: 0,
            irr: [0; 8],
            isr: [0; 8],
            tmr: [0; 8],
        }
    }
}

impl State {
    pub(crate) fn highest(bits: &[u32; 8]) -> Option<u8> {
        bits.iter().enumerate().rev().find_map(|(i, word)| {
            (*word != 0).then(|| (i * 32 + 31 - word.leading_zeros() as usize) as u8)
        })
    }
    pub(crate) fn ppr(&self) -> u8 {
        let service = Self::highest(&self.isr).unwrap_or(0) & 0xf0;
        if self.tpr & 0xf0 >= service {
            self.tpr
        } else {
            service
        }
    }
    pub(crate) fn eligible(&self) -> Option<u8> {
        let vector = Self::highest(&self.irr)?;
        (self.global_enabled && self.svr & 0x100 != 0 && (vector & 0xf0) > (self.ppr() & 0xf0))
            .then_some(vector)
    }
}

impl LocalApic {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(PairState::default())))
    }
    pub fn handle(&self) -> LocalApicHandle {
        LocalApicHandle(Arc::clone(&self.0))
    }
    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()).local)
    }
}
impl Default for LocalApic {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalApicHandle {
    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()).local)
    }
    /// Reset only the Local APIC; a level request awaiting EOI vetoes reset.
    pub fn reset(&self) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset_local()
    }
    /// Atomically reset the pair, discarding all accepted requests.
    pub fn reset_pair(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = PairState::default();
    }
    /// Disable is rejected while a level request awaits EOI.
    pub fn set_global_enabled(&self, enabled: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_enabled(enabled)
    }
    /// Record a fixed edge. Repeated requests coalesce in the IRR bit.
    pub fn request_fixed_edge(&self, vector: u8) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .edge(vector)
    }
    /// Observe the highest pending vector without consuming it.
    pub fn pending(&self) -> Option<u8> {
        self.with_state(|s| State::highest(&s.irr))
    }
    /// Observe the highest eligible vector without consuming it.
    pub fn eligible(&self) -> Option<u8> {
        self.with_state(|s| s.eligible())
    }
    /// Move one eligible IRR bit to ISR. This does not execute a guest handler.
    pub fn acknowledge(&self) -> Option<u8> {
        self.with_state(|s| {
            let v = s.eligible()?;
            let index = usize::from(v / 32);
            let bit = 1 << (v % 32);
            s.irr[index] &= !bit;
            s.isr[index] |= bit;
            Some(v)
        })
    }

    /// Commit the selected request and CPU entry together. The caller must
    /// serialize all APIC operations across selection and delivery.
    pub(crate) fn acknowledge_vector(&self, vector: u8, publish: impl FnOnce()) -> bool {
        self.with_state(|s| {
            if s.eligible() != Some(vector) {
                return false;
            }
            let index = usize::from(vector / 32);
            let bit = 1 << (vector % 32);
            s.irr[index] &= !bit;
            s.isr[index] |= bit;
            publish();
            true
        })
    }
}

/// Execute the bounded CPU MSR operation and apply any retired write before
/// returning control to the machine. The caller owns the CPU and memory and
/// serializes this entire call, including fetch, with all pair operations.
pub fn step_apic_base_msr(
    cpu: &mut od_isa::msr::ApicBaseCpu,
    apic: &LocalApicHandle,
    memory: &mut impl od_isa::GuestMemory,
    context: od_isa::AccessContext,
    budget: u64,
    stop: bool,
) -> Result<od_isa::msr::ApicBaseExit, DeviceError> {
    let enabled = apic.with_state(|s| s.global_enabled);
    if enabled != (cpu.apic_base() == od_isa::msr::ApicBaseCpu::ENABLED) {
        return Err(DeviceError::UnsupportedConfiguration);
    }
    let mut candidate = cpu.clone();
    let outcome = candidate.step(memory, context, budget, stop);
    let mut pair = apic.0.lock().unwrap_or_else(|e| e.into_inner());
    if let od_isa::msr::ApicBaseExit::Write(effect) = outcome {
        pair.set_enabled(effect.new == od_isa::msr::ApicBaseCpu::ENABLED)?;
    }
    *cpu = candidate;
    Ok(outcome)
}

/// Execute the opt-in Long64 CR8 profile against this APIC's TPR. The guest
/// fetch runs outside the device lock; a retired write effect is applied
/// synchronously before this function returns. The caller must serialize all
/// operations on this APIC, including operations initiated during guest fetch,
/// with the entire call; the handle's mutex does not provide that ordering.
pub fn step_local_apic_cr8(
    state: &mut od_isa::LongModeState,
    apic: &LocalApicHandle,
    memory: &mut impl od_isa::GuestMemory,
    context: od_isa::AccessContext,
    budget: u64,
    stop: bool,
) -> od_isa::cr8::Cr8Exit {
    if stop {
        return od_isa::cr8::Cr8Exit::Stop;
    }
    if budget == 0 {
        return od_isa::cr8::Cr8Exit::Budget;
    }
    let tpr = apic.with_state(|s| (s.global_enabled && s.svr & 0x100 != 0).then_some(s.tpr));
    let Some(tpr) = tpr else {
        return od_isa::cr8::Cr8Exit::Unsupported;
    };
    let outcome = od_isa::cr8::step_cr8(
        od_isa::cr8::Cr8Long64V1,
        state,
        memory,
        context,
        tpr,
        budget,
        stop,
    );
    if let od_isa::cr8::Cr8Exit::Write(effect) = &outcome {
        apic.with_state(|s| s.tpr = effect.new_tpr);
    }
    outcome
}

fn validate(a: DeviceAccess) -> Result<u64, DeviceError> {
    if a.space != IoSpace::Mmio {
        return Err(DeviceError::UnsupportedSpace);
    }
    if a.width != AccessWidth::Dword {
        return Err(DeviceError::UnsupportedWidth);
    }
    if a.offset % 16 != 0 {
        return Err(DeviceError::Misaligned);
    }
    Ok(a.offset)
}
fn bank(offset: u64, base: u64) -> Option<usize> {
    if (base..base + 0x80).contains(&offset) {
        Some(((offset - base) / 16) as usize)
    } else {
        None
    }
}
impl BusDevice for LocalApic {
    fn read(&mut self, a: DeviceAccess) -> Result<u64, DeviceError> {
        let offset = validate(a)?;
        self.with_state(|s| {
            if !s.global_enabled {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            let value = match offset {
                0x20 => 0, // fixed APIC ID
                0x80 => u32::from(s.tpr),
                0xa0 => u32::from(s.ppr()),
                0xb0 => return Err(DeviceError::WriteOnly),
                0xf0 => u32::from(s.svr),
                0x300 => s.icr_low,
                0x310 => s.icr_high,
                _ if bank(offset, 0x100).is_some() => s.isr[bank(offset, 0x100).unwrap()],
                _ if bank(offset, 0x180).is_some() => s.tmr[bank(offset, 0x180).unwrap()],
                _ if bank(offset, 0x200).is_some() => s.irr[bank(offset, 0x200).unwrap()],
                _ => return Err(DeviceError::UnknownOffset),
            };
            Ok(u64::from(value))
        })
    }
    fn write(&mut self, a: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let offset = validate(a)?;
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        {
            if !pair.local.global_enabled {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            match offset {
                0x80 if value <= 0xff => {
                    pair.local.tpr = value as u8;
                    Ok(())
                }
                0xf0 if value & !0x1ff == 0 => {
                    if value & 0x100 == 0 && pair.route.remote_irr {
                        return Err(DeviceError::UnsupportedConfiguration);
                    }
                    pair.local.svr = value as u16;
                    Ok(())
                }
                0xb0 if value == 0 => pair.eoi(),
                // Fixed, edge-triggered, asserted self-IPI. Destination mode
                // is ignored for self shorthand. Delivery completes here.
                0x300
                    if value & !0x4_58ff == 0
                        && value & 0x4_4000 == 0x4_4000
                        && value & 0xff >= 16 =>
                {
                    // Delivery Status is device-owned and remains Idle in
                    // this synchronous self-delivery profile.
                    let command = value & !(1 << 12);
                    pair.edge(command as u8)?;
                    pair.local.icr_low = command as u32;
                    Ok(())
                }
                0x310 if value & !0xff00_0000 == 0 => {
                    pair.local.icr_high = value as u32;
                    Ok(())
                }
                0x80 | 0xb0 | 0xf0 | 0x300 | 0x310 => Err(DeviceError::UnsupportedConfiguration),
                0x20 | 0xa0 => Err(DeviceError::ReadOnly),
                _ if bank(offset, 0x100).is_some()
                    || bank(offset, 0x180).is_some()
                    || bank(offset, 0x200).is_some() =>
                {
                    Err(DeviceError::ReadOnly)
                }
                _ => Err(DeviceError::UnknownOffset),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    };

    struct MsrCode(Vec<u8>);
    impl od_isa::GuestMemory for MsrCode {
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: od_isa::AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, od_isa::MemoryError> {
            unreachable!()
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: od_isa::AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, od_isa::MemoryError> {
            unreachable!()
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: od_isa::AccessContext,
            _: od_isa::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, od_isa::MemoryError> {
            unreachable!()
        }
        fn mfence(&mut self, _: od_isa::AccessContext) -> Result<(), od_isa::MemoryError> {
            unreachable!()
        }
        fn fetch(
            &mut self,
            address: u64,
            _: od_isa::AccessContext,
        ) -> Result<u8, od_isa::MemoryError> {
            self.0
                .get(address as usize)
                .copied()
                .ok_or_else(|| od_isa::MemoryError::Backend("fetch".into()))
        }
        fn read(
            &mut self,
            _: u64,
            _: od_isa::AccessContext,
            _: &mut [u8],
        ) -> Result<(), od_isa::MemoryError> {
            unreachable!()
        }
        fn write(
            &mut self,
            _: u64,
            _: od_isa::AccessContext,
            _: &[u8],
        ) -> Result<(), od_isa::MemoryError> {
            unreachable!()
        }
    }

    #[test]
    fn msr_disable_veto_does_not_retire_or_change_pair() {
        use od_isa::msr::ApicBaseCpu;
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        {
            let mut pair = apic.0.lock().unwrap();
            pair.set_low(crate::apic_pair::LEVEL | 0x51).unwrap();
            pair.set_line(true).unwrap();
        }
        let before_pair = apic.0.lock().unwrap().clone();
        let mut cpu = ApicBaseCpu::new(od_isa::LongModeState::default());
        cpu.state.gpr[od_isa::Register::Rcx as usize] = 0x1b;
        cpu.state.gpr[od_isa::Register::Rax as usize] = ApicBaseCpu::DISABLED;
        let before_cpu = cpu.clone();
        let context = od_isa::AccessContext {
            privilege: od_isa::PrivilegeLevel::Ring0,
            ..Default::default()
        };
        assert_eq!(
            step_apic_base_msr(
                &mut cpu,
                &apic,
                &mut MsrCode(vec![0x0f, 0x30]),
                context,
                1,
                false
            ),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(cpu, before_cpu);
        assert_eq!(*apic.0.lock().unwrap(), before_pair);
    }

    #[test]
    fn retired_msr_effect_applies_before_next_device_access() {
        use od_isa::msr::{ApicBaseCpu, ApicBaseExit};
        let (mut bus, apic) = setup();
        let mut cpu = ApicBaseCpu::new(od_isa::LongModeState::default());
        cpu.state.gpr[od_isa::Register::Rcx as usize] = 0x1b;
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x40).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x40));
        apic.request_fixed_edge(0x60).unwrap();
        let context = od_isa::AccessContext {
            privilege: od_isa::PrivilegeLevel::Ring0,
            ..Default::default()
        };
        for target in [
            ApicBaseCpu::ENABLED,
            ApicBaseCpu::DISABLED,
            ApicBaseCpu::ENABLED,
        ] {
            cpu.state.rip = 0;
            cpu.state.gpr[od_isa::Register::Rax as usize] = target;
            assert!(matches!(
                step_apic_base_msr(
                    &mut cpu,
                    &apic,
                    &mut MsrCode(vec![0x0f, 0x30]),
                    context,
                    1,
                    false
                ),
                Ok(ApicBaseExit::Write(_))
            ));
            if target == ApicBaseCpu::DISABLED {
                assert_eq!(
                    read(&mut bus, 0xf0),
                    Err(error(DeviceError::UnsupportedConfiguration))
                );
                assert_eq!(apic.pending(), None);
            } else if apic.pending().is_some() {
                assert_eq!(apic.eligible(), Some(0x60));
            } else {
                assert_eq!(read(&mut bus, 0xf0), Ok(0xff));
                assert_eq!(read(&mut bus, 0x120), Ok(0));
            }
        }
    }

    #[test]
    fn cr8_tpr_controls_self_ipi_eligibility_and_preserves_pending_service() {
        use od_isa::{CpuActivity, StiShadow, cr8::Cr8Exit};
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x80, 0x4f).unwrap();
        write(&mut bus, 0x300, 0x4_4000 | 0x51).unwrap();
        write(&mut bus, 0x300, 0x4_4000 | 0x41).unwrap();

        let mut state = od_isa::LongModeState::default();
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        state.gpr[od_isa::Register::Rax as usize] = 5;
        let context = od_isa::AccessContext {
            privilege: od_isa::PrivilegeLevel::Ring0,
            ..Default::default()
        };
        assert_eq!(
            step_local_apic_cr8(
                &mut state,
                &apic,
                &mut MsrCode(vec![0x44, 0x0f, 0x22, 0xc0]),
                context,
                1,
                false,
            ),
            Cr8Exit::Write(od_isa::cr8::Cr8TprEffect {
                old_tpr: 0x4f,
                new_tpr: 0x50,
            })
        );
        assert_eq!(read(&mut bus, 0x80), Ok(0x50));
        assert_eq!(read(&mut bus, 0xa0), Ok(0x50));
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(apic.eligible(), None);

        state.rip = 0;
        assert_eq!(
            step_local_apic_cr8(
                &mut state,
                &apic,
                &mut MsrCode(vec![0x44, 0x0f, 0x20, 0xc0]),
                context,
                1,
                false,
            ),
            Cr8Exit::Read
        );
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 5);

        state.rip = 0;
        state.gpr[od_isa::Register::Rax as usize] = 4;
        assert!(matches!(
            step_local_apic_cr8(
                &mut state,
                &apic,
                &mut MsrCode(vec![0x44, 0x0f, 0x22, 0xc0]),
                context,
                1,
                false,
            ),
            Cr8Exit::Write(_)
        ));
        assert_eq!(read(&mut bus, 0x80), Ok(0x40));
        assert_eq!(apic.eligible(), Some(0x51));
        assert_eq!(apic.acknowledge(), Some(0x51));
        assert_eq!(read(&mut bus, 0x120), Ok(1 << 17));
        assert_eq!(read(&mut bus, 0xa0), Ok(0x50));
        assert_eq!(apic.pending(), Some(0x41));

        state.rip = 0;
        state.gpr[od_isa::Register::Rax as usize] = 0;
        assert!(matches!(
            step_local_apic_cr8(
                &mut state,
                &apic,
                &mut MsrCode(vec![0x44, 0x0f, 0x22, 0xc0]),
                context,
                1,
                false,
            ),
            Cr8Exit::Write(_)
        ));
        assert_eq!(read(&mut bus, 0x80), Ok(0));
        assert_eq!(read(&mut bus, 0x120), Ok(1 << 17));
        assert_eq!(apic.eligible(), None); // ISR class 5 still controls PPR.
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(read(&mut bus, 0x120), Ok(0));
        assert_eq!(apic.eligible(), Some(0x41));
    }

    #[test]
    fn cr8_admission_stop_budget_and_disabled_apic_do_not_fetch() {
        use od_isa::{CpuActivity, StiShadow, cr8::Cr8Exit};
        let (_bus, apic) = setup();
        let mut state = od_isa::LongModeState::default();
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        let context = od_isa::AccessContext {
            privilege: od_isa::PrivilegeLevel::Ring0,
            ..Default::default()
        };
        let mut code = MsrCode(vec![]);
        assert_eq!(
            step_local_apic_cr8(&mut state, &apic, &mut code, context, 0, false),
            Cr8Exit::Budget
        );
        assert_eq!(
            step_local_apic_cr8(&mut state, &apic, &mut code, context, 1, true),
            Cr8Exit::Stop
        );
        apic.with_state(|s| s.svr = 0xff);
        assert_eq!(
            step_local_apic_cr8(&mut state, &apic, &mut code, context, 1, false),
            Cr8Exit::Unsupported
        );
        apic.set_global_enabled(false).unwrap();
        assert_eq!(
            step_local_apic_cr8(&mut state, &apic, &mut code, context, 1, false),
            Cr8Exit::Unsupported
        );
        assert_eq!(state.rip, 0);
    }

    #[test]
    fn cr8_rejected_operations_preserve_cpu_and_apic_state() {
        use od_isa::{CpuActivity, PrivilegeLevel, StiShadow, cr8::Cr8Exit};
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x80, 0x3f).unwrap();
        write(&mut bus, 0x300, 0x4_4000 | 0x51).unwrap();
        let mut cpu = od_isa::LongModeState::default();
        cpu.activity = Some(CpuActivity::Running);
        cpu.sti_shadow = Some(StiShadow::Clear);
        cpu.gpr[od_isa::Register::Rax as usize] = 0x10;
        let context = od_isa::AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..Default::default()
        };
        let before_cpu = cpu.clone();
        let before_apic = apic.with_state(|s| s.clone());
        for (bytes, privilege, expected_vector) in [
            (vec![0x44, 0x0f, 0x22, 0xc0], PrivilegeLevel::Ring0, 13),
            (vec![0x44, 0x0f, 0x20, 0xc0], PrivilegeLevel::Ring3, 13),
            (vec![0xf0, 0x44, 0x0f, 0x22, 0xc0], PrivilegeLevel::Ring0, 6),
        ] {
            let actual = step_local_apic_cr8(
                &mut cpu,
                &apic,
                &mut MsrCode(bytes),
                od_isa::AccessContext {
                    privilege,
                    ..context
                },
                1,
                false,
            );
            assert!(matches!(actual, Cr8Exit::Fault(f) if f.vector == expected_vector));
            assert_eq!(cpu, before_cpu);
            assert_eq!(apic.with_state(|s| s.clone()), before_apic);
        }
        assert!(matches!(
            step_local_apic_cr8(
                &mut cpu,
                &apic,
                &mut MsrCode(vec![0x44, 0x0f]),
                context,
                1,
                false
            ),
            Cr8Exit::Memory(_)
        ));
        assert_eq!(cpu, before_cpu);
        assert_eq!(apic.with_state(|s| s.clone()), before_apic);
    }

    fn setup() -> (MachineBus, LocalApicHandle) {
        let device = LocalApic::new();
        let handle = device.handle();
        let base = GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE);
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(base, LOCAL_APIC_MMIO_SIZE).unwrap(),
                MemoryRegionKind::Mmio,
            )],
        )
        .unwrap();
        let bus = MachineBus::new(
            map,
            RamLimits::new(0),
            vec![Box::new(device)],
            vec![MmioBinding {
                region_start: base,
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        (bus, handle)
    }
    fn read(bus: &mut MachineBus, offset: u64) -> Result<u64, BusAccessError> {
        bus.read_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
        )
    }
    fn write(bus: &mut MachineBus, offset: u64, value: u64) -> Result<(), BusAccessError> {
        bus.write_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
            value,
            |_| {},
        )
    }
    fn error(error: DeviceError) -> BusAccessError {
        BusAccessError::Device {
            device_index: 0,
            source: error,
        }
    }

    #[test]
    fn icr_self_ipi_delivery_retrigger_and_priority() {
        let (mut bus, apic) = setup();
        const SELF_ASSERT: u64 = 0x4_4000;
        assert_eq!(read(&mut bus, 0x300), Ok(0));
        assert_eq!(read(&mut bus, 0x310), Ok(0));
        write(&mut bus, 0x310, 0xab00_0000).unwrap();
        assert_eq!(read(&mut bus, 0x310), Ok(0xab00_0000));
        assert_eq!(apic.pending(), None);

        // Software disable does not prevent an IPI from entering IRR.
        write(&mut bus, 0x300, SELF_ASSERT | 0x1000 | 0x41).unwrap();
        assert_eq!(read(&mut bus, 0x300), Ok(SELF_ASSERT | 0x41));
        assert_eq!(read(&mut bus, 0x220), Ok(1 << 1));
        assert_eq!(apic.eligible(), None);
        assert_eq!(apic.acknowledge(), None);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x80, 0x40).unwrap();
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0x80, 0).unwrap();
        // Two commands before acknowledge coalesce.
        write(&mut bus, 0x300, SELF_ASSERT | 0x41).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x41));
        assert_eq!(read(&mut bus, 0x220), Ok(0));
        assert_eq!(read(&mut bus, 0x120), Ok(1 << 1));
        // The same command after acknowledge queues another request.
        write(&mut bus, 0x300, SELF_ASSERT | 0x41).unwrap();
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0x300, SELF_ASSERT | 0x800 | 0x62).unwrap();
        assert_eq!(read(&mut bus, 0x300), Ok(SELF_ASSERT | 0x800 | 0x62));
        assert_eq!(apic.acknowledge(), Some(0x62));
        assert_eq!(read(&mut bus, 0x130), Ok(1 << 2));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x41));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x120), Ok(0));
    }

    #[test]
    fn icr_rejections_are_atomic_and_global_reset_clears_registers() {
        let (mut bus, apic) = setup();
        const SELF_ASSERT: u64 = 0x4_4000;
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x310, 0x7f00_0000).unwrap();
        write(&mut bus, 0x300, SELF_ASSERT | 0x50).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x50));
        write(&mut bus, 0x300, SELF_ASSERT | 0x60).unwrap();
        let before = apic.with_state(|s| s.clone());
        for invalid in [
            SELF_ASSERT | 15,                        // illegal vector
            SELF_ASSERT | 0x100 | 0x60,              // delivery mode
            0x4000 | 0x60,                           // no shorthand
            0x8_4000 | 0x60,                         // another shorthand
            SELF_ASSERT | 0x8000 | 0x60,             // level-triggered
            0x4_0000 | 0x60,                         // deassert
            SELF_ASSERT | 0x20_0000 | 0x60,          // reserved
            SELF_ASSERT | 0x1000 | 0x20_0000 | 0x60, // status does not legalize reserved bits
        ] {
            assert_eq!(
                write(&mut bus, 0x300, invalid),
                Err(error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(apic.with_state(|s| s.clone()), before);
        }
        assert_eq!(
            write(&mut bus, 0x310, 1),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(
            write(&mut bus, 0x304, SELF_ASSERT | 0x60),
            Err(error(DeviceError::Misaligned))
        );
        assert_eq!(
            bus.write_physical(
                GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0x300),
                AccessWidth::Byte,
                0x60,
                |_| {}
            ),
            Err(error(DeviceError::UnsupportedWidth))
        );
        assert_eq!(apic.with_state(|s| s.clone()), before);
        apic.set_global_enabled(false).unwrap();
        let disabled = apic.with_state(|s| s.clone());
        assert_eq!(
            write(&mut bus, 0x300, SELF_ASSERT | 0x60),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(
            write(&mut bus, 0x310, 0x1200_0000),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(
            read(&mut bus, 0x300),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(apic.with_state(|s| s.clone()), disabled);
        apic.set_global_enabled(true).unwrap();
        assert_eq!(read(&mut bus, 0x300), Ok(0));
        assert_eq!(read(&mut bus, 0x310), Ok(0));
        assert_eq!(apic.pending(), None);
        write(&mut bus, 0x300, SELF_ASSERT | 0xff).unwrap();
        apic.reset().unwrap();
        assert_eq!(read(&mut bus, 0x300), Ok(0));
        assert_eq!(read(&mut bus, 0x310), Ok(0));
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn priority_acknowledge_retrigger_eoi_and_reset() {
        let (mut bus, apic) = setup();
        assert_eq!(read(&mut bus, 0x20), Ok(0));
        assert_eq!(read(&mut bus, 0xf0), Ok(0xff));
        apic.request_fixed_edge(0x41).unwrap();
        apic.request_fixed_edge(0x52).unwrap();
        assert_eq!(apic.pending(), Some(0x52));
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x80, 0x5f).unwrap();
        assert_eq!(read(&mut bus, 0xa0), Ok(0x5f));
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0x80, 0x10).unwrap();
        assert_eq!(apic.eligible(), Some(0x52));
        assert_eq!(apic.eligible(), Some(0x52));
        assert_eq!(apic.acknowledge(), Some(0x52));
        assert_eq!(apic.acknowledge(), None);
        assert_eq!(read(&mut bus, 0x120), Ok(1 << 18));
        assert_eq!(read(&mut bus, 0xa0), Ok(0x50));
        apic.request_fixed_edge(0x52).unwrap();
        assert_eq!(apic.pending(), Some(0x52));
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0x80, 0x5f).unwrap();
        assert_eq!(read(&mut bus, 0xa0), Ok(0x5f));
        assert_eq!(apic.eligible(), None);
        assert_eq!(apic.acknowledge(), None);
        write(&mut bus, 0x80, 0x10).unwrap();
        apic.request_fixed_edge(0x70).unwrap();
        write(&mut bus, 0xf0, 0xff).unwrap();
        assert_eq!(read(&mut bus, 0x120), Ok(1 << 18));
        assert_eq!(read(&mut bus, 0x230), Ok(1 << 16));
        assert_eq!(apic.pending(), Some(0x70));
        assert_eq!(apic.eligible(), None);
        assert_eq!(apic.acknowledge(), None);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        assert_eq!(apic.eligible(), Some(0x70));
        assert_eq!(apic.acknowledge(), Some(0x70));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(read(&mut bus, 0xa0), Ok(0x50));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x52));
        assert_eq!(read(&mut bus, 0x1a0), Ok(0));
        apic.reset().unwrap();
        assert_eq!(apic.pending(), None);
        assert_eq!(apic.eligible(), None);
        assert_eq!(read(&mut bus, 0x120), Ok(0));
        assert_eq!(read(&mut bus, 0x80), Ok(0));
        assert_eq!(read(&mut bus, 0xf0), Ok(0xff));
    }

    #[test]
    fn global_disable_resets_and_requires_software_reenable() {
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x40).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x40));
        apic.request_fixed_edge(0x60).unwrap();
        apic.set_global_enabled(true).unwrap();
        assert_eq!(apic.eligible(), Some(0x60));
        assert_eq!(read(&mut bus, 0x120), Ok(1));
        apic.set_global_enabled(false).unwrap();
        assert_eq!(apic.pending(), None);
        assert_eq!(apic.acknowledge(), None);
        assert_eq!(
            read(&mut bus, 0xf0),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(
            write(&mut bus, 0xf0, 0x1ff),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(
            apic.request_fixed_edge(0x60),
            Err(DeviceError::UnsupportedConfiguration)
        );
        apic.set_global_enabled(false).unwrap();
        apic.set_global_enabled(true).unwrap();
        assert_eq!(read(&mut bus, 0x120), Ok(0));
        assert_eq!(read(&mut bus, 0xf0), Ok(0xff));
        apic.request_fixed_edge(0x60).unwrap();
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        assert_eq!(apic.eligible(), Some(0x60));
    }

    #[test]
    fn unsupported_accesses_are_atomic() {
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x40).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x40));
        apic.request_fixed_edge(0x60).unwrap();
        write(&mut bus, 0x80, 0x2f).unwrap();
        assert_eq!(
            write(&mut bus, 0x80, 0x100),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(read(&mut bus, 0x80), Ok(0x2f));
        assert_eq!(
            apic.request_fixed_edge(15),
            Err(DeviceError::UnsupportedConfiguration)
        );
        for (offset, value, expected) in [
            (0xf0, 0x200, DeviceError::UnsupportedConfiguration),
            (0xb0, 1, DeviceError::UnsupportedConfiguration),
            (0x20, 0, DeviceError::ReadOnly),
            (0x120, 0, DeviceError::ReadOnly),
            (0x1a0, 0, DeviceError::ReadOnly),
            (0x230, 0, DeviceError::ReadOnly),
            (0x30, 0, DeviceError::UnknownOffset),
            (0x300, 0, DeviceError::UnsupportedConfiguration),
            (0x84, 0, DeviceError::Misaligned),
        ] {
            assert_eq!(write(&mut bus, offset, value), Err(error(expected)));
        }
        assert_eq!(read(&mut bus, 0xb0), Err(error(DeviceError::WriteOnly)));
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0x80),
                AccessWidth::Byte
            ),
            Err(error(DeviceError::UnsupportedWidth))
        );
        assert_eq!(apic.pending(), Some(0x60));
        assert_eq!(apic.eligible(), Some(0x60));
        assert_eq!(read(&mut bus, 0x120), Ok(1));
        assert_eq!(read(&mut bus, 0xa0), Ok(0x40));
        assert_eq!(read(&mut bus, 0xf0), Ok(0x1ff));
    }
}
