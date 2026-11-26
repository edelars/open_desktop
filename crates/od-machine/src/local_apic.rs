//! A deliberately incomplete xAPIC register and fixed interrupt model, one Local APIC per vCPU
//! (S6-11; one by default).
//!
//! The Version register (S4-25f, S7-32 slice M) advertises an integrated APIC with five LVT
//! entries (Timer, Performance Monitoring Counters, LINT0, LINT1, Error); the Thermal Sensor and
//! CMCI entries do not exist. The PMC, LINT0/LINT1/Error LVT, DFR, LDR and ESR only store state:
//! there is no counter-overflow source (no CPUID profile declares a PMU), no LINT pin or error
//! source, and no CPU interrupt delivery is performed. An ICR write raises a Fixed edge request in the IRR of its
//! targets (S6-12: self, a physical APIC ID, all, all but self). An NMI (S6-12b: a physical APIC ID
//! or all but self) is remembered as one pending latch in the LAPIC of every target. INIT assert and
//! SIPI (S6-13) are queued as pending events in the LAPIC of the target, which the target applies
//! itself on its own boundary ([`LocalApicHandle::take_startup`]); INIT deassert is accepted and
//! changes nothing.
//! Optional ESR reporting for an invalid vector written to a masked LVT is not modelled.
use crate::apic_pair::{CpuApic, MASK, PERIODIC, PairState, PendingStartup, Timer};
use crate::lock_wait_metrics::MeasuredMutex;
use crate::vcpu::{BOOT_VCPU_INDEX, MAX_VCPUS};
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use std::sync::Arc;

pub const LOCAL_APIC_MMIO_BASE: u64 = 0xfee0_0000;
pub const LOCAL_APIC_MMIO_SIZE: u64 = 0x1000;

/// The Local APICs of the board, one per vCPU, behind the one bus window `0xFEE0_0000` (S6-11).
/// A window access reaches the LAPIC of the vCPU that makes it: the bus passes that vCPU's index
/// to [`BusDevice::read_from_cpu`]/[`BusDevice::write_from_cpu`]; [`BusDevice::read`] and
/// [`BusDevice::write`] are the accesses of the boot processor.
#[derive(Clone, Debug)]
pub struct LocalApic(Arc<MeasuredMutex<PairState>>);

/// Control operations address the same state as the device on the bus, for the LAPIC of one
/// vCPU (the second field). [`LocalApic::handle`] is the boot processor's.
#[derive(Clone, Debug)]
pub struct LocalApicHandle(pub(crate) Arc<MeasuredMutex<PairState>>, pub(crate) usize);

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
    /// LINT0/LINT1/Error LVT (S4-25f): storage and mask only, no source.
    pub(crate) lint0: u32,
    pub(crate) lint1: u32,
    pub(crate) error_lvt: u32,
    /// LVT Performance Monitoring Counters (S7-32 slice M): storage and mask only; there is no
    /// counter-overflow source, since no CPUID profile declares a PMU.
    pub(crate) pmc_lvt: u32,
    /// Destination Format and Logical Destination registers: storage only.
    pub(crate) dfr: u32,
    pub(crate) ldr: u32,
    /// Latched Error Status Register and errors accumulated since the last
    /// ESR write. Nothing sets `esr_pending` in this model.
    pub(crate) esr: u32,
    pub(crate) esr_pending: u32,
    /// An INIT assert waits for the target to apply it (S6-13, SMP_CONTRACT.md 3). One bit: INIT
    /// is level-triggered and a second assert before the first is applied is the same event.
    pub(crate) pending_init: bool,
    /// A SIPI vector waits for the target to apply it. The first of several wins: the target
    /// leaves `WaitForSipi` on it and ignores the rest. An INIT assert drops a SIPI accepted
    /// before it (SMP_CONTRACT.md 3); a SIPI accepted after it is kept.
    pub(crate) pending_sipi: Option<u8>,
    /// An NMI waits for the target to deliver it through vector 2 (S6-12b, SMP_CONTRACT.md 7).
    /// One latch: requests that arrive before the target takes it, or while the target's NMI
    /// delivery is blocked until `IRETQ`, are the same event, so at most one NMI is remembered
    /// and the rest are lost. Reset to `false` with the rest of the state by a LAPIC reset, an
    /// INIT reset and a global disable.
    pub(crate) pending_nmi: bool,
}

/// Version register: version 0x14 (integrated APIC), Max LVT entry 4 (five LVT entries: Timer,
/// Performance Monitoring Counters, LINT0, LINT1, Error; the P6 layout of SDM Vol. 3A 13.4.8,
/// S7-32 slice M), no EOI-broadcast suppression. Thermal Sensor and CMCI are absent.
pub(crate) const VERSION: u32 = 0x0004_0014;
const LVT_RESET: u32 = MASK;
const DFR_RESET: u32 = 0xffff_ffff;
/// Writable bits of LINT0/LINT1: vector, delivery mode, polarity, trigger,
/// mask, plus the read-only Delivery Status (12) and Remote IRR (14), which
/// are accepted and dropped.
const LINT_BITS: u32 = 0x1_f7ff;
/// Error LVT: vector, mask and the read-only Delivery Status (12).
const ERROR_LVT_BITS: u32 = 0x1_10ff;
/// LVT Performance Monitoring Counters: vector, delivery mode and mask, plus the read-only
/// Delivery Status (12), accepted and dropped. The entry has no polarity, Remote IRR or trigger
/// mode (SDM Vol. 3A Figure 13-8).
const PMC_LVT_BITS: u32 = 0x1_17ff;
const LVT_READ_ONLY_BITS: u32 = 0x5000;

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
            lint0: LVT_RESET,
            lint1: LVT_RESET,
            error_lvt: LVT_RESET,
            pmc_lvt: LVT_RESET,
            dfr: DFR_RESET,
            ldr: 0,
            esr: 0,
            esr_pending: 0,
            pending_init: false,
            pending_sipi: None,
            pending_nmi: false,
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
    /// Move one request from IRR to ISR. The caller has already selected it.
    pub(crate) fn move_to_service(&mut self, vector: u8) {
        let index = usize::from(vector / 32);
        let bit = 1 << (vector % 32);
        self.irr[index] &= !bit;
        self.isr[index] |= bit;
    }
}

/// Result of an interrupt acknowledge cycle (S4-25e).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptAcknowledge {
    /// The highest eligible request moved from IRR to ISR.
    Fixed(u8),
    /// No request was eligible when the cycle ran, for example because it was
    /// withdrawn after selection. The vector is `SVR[7:0]`; IRR, ISR and TMR
    /// are untouched, so the guest must not signal EOI for it.
    Spurious(u8),
}

impl LocalApic {
    pub fn new() -> Self {
        Self(Arc::new(MeasuredMutex::new(PairState::default())))
    }
    /// `count` Local APICs without timers, APIC IDs `0..count`; `count` is `1..=MAX_VCPUS`.
    pub fn with_cpus(count: usize) -> Result<Self, DeviceError> {
        Self::checked_count(count)?;
        let cpus = vec![CpuApic::default(); count];
        Ok(Self(Arc::new(MeasuredMutex::new(PairState::new(cpus)))))
    }
    /// Create an opt-in one-shot/periodic timer with its own caller-driven
    /// clock. This does not connect a vCPU clock, NMI, MOV-SS, a Linux profile,
    /// or a general scheduler.
    pub fn with_timer(clock: crate::VirtualClock, timer_hz: u64) -> Result<Self, DeviceError> {
        let cpu = CpuApic {
            timer: Some(Timer::new(clock, timer_hz)?),
            ..CpuApic::default()
        };
        Ok(Self(Arc::new(MeasuredMutex::new(PairState::new(vec![
            cpu,
        ])))))
    }
    /// Create an opt-in one-shot/periodic timer observing a vCPU-owned clock.
    /// The APIC cannot advance this clock; its vCPU session remains the owner.
    pub fn with_shared_timer(
        clock: crate::VirtualClockHandle,
        timer_hz: u64,
    ) -> Result<Self, DeviceError> {
        Self::with_shared_timers(clock, timer_hz, 1)
    }
    /// `count` Local APICs, each with its own timer and event queue observing the same clock
    /// (the board clock); `count` is `1..=MAX_VCPUS`.
    pub fn with_shared_timers(
        clock: crate::VirtualClockHandle,
        timer_hz: u64,
        count: usize,
    ) -> Result<Self, DeviceError> {
        Self::checked_count(count)?;
        let cpus = (0..count)
            .map(|_| {
                Ok(CpuApic {
                    timer: Some(Timer::new_shared(clock.clone(), timer_hz)?),
                    ..CpuApic::default()
                })
            })
            .collect::<Result<Vec<_>, DeviceError>>()?;
        Ok(Self(Arc::new(MeasuredMutex::new(PairState::new(cpus)))))
    }
    fn checked_count(count: usize) -> Result<(), DeviceError> {
        if (1..=MAX_VCPUS).contains(&count) {
            Ok(())
        } else {
            Err(DeviceError::UnsupportedConfiguration)
        }
    }
    /// Handle of the boot processor's LAPIC.
    pub fn handle(&self) -> LocalApicHandle {
        LocalApicHandle(Arc::clone(&self.0), BOOT_VCPU_INDEX)
    }
    /// Handle of the LAPIC of vCPU `cpu`, or `None` when the board has no such vCPU.
    pub fn handle_for(&self, cpu: usize) -> Option<LocalApicHandle> {
        self.handle().for_cpu(cpu)
    }
}
impl Default for LocalApic {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalApicHandle {
    /// Handle of the LAPIC of vCPU `cpu` of the same board, or `None` when there is no such vCPU.
    pub fn for_cpu(&self, cpu: usize) -> Option<LocalApicHandle> {
        let count = self.0.lock().unwrap_or_else(|e| e.into_inner()).cpus.len();
        (cpu < count).then(|| LocalApicHandle(Arc::clone(&self.0), cpu))
    }
    /// Number of Local APICs (vCPUs) of the board this handle belongs to.
    pub fn cpu_count(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).cpus.len()
    }
    /// Index of the vCPU whose LAPIC this handle addresses (its APIC ID).
    pub fn cpu_index(&self) -> usize {
        self.1
    }
    fn with_state_pair<R>(&self, f: impl FnOnce(&crate::apic_pair::CpuApic) -> R) -> R {
        f(&self.0.lock().unwrap_or_else(|e| e.into_inner()).cpus[self.1])
    }
    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()).cpus[self.1].local)
    }
    /// The INIT and SIPI waiting in this LAPIC, without consuming them (S6-13).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pending_startup(&self) -> PendingStartup {
        self.with_state(|s| PendingStartup {
            init: s.pending_init,
            sipi: s.pending_sipi,
        })
    }
    /// Take the pending INIT and SIPI of this LAPIC for the owning vCPU to apply on its own
    /// boundary (S6-13). A pending INIT also puts this LAPIC into its INIT-reset state (see
    /// [`PairState::take_startup`]). On an error nothing is taken.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn take_startup(&self) -> Result<PendingStartup, DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_startup(self.1)
    }
    /// Whether an NMI waits in this LAPIC, without consuming it (S6-12b).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pending_nmi(&self) -> bool {
        self.with_state(|s| s.pending_nmi)
    }
    /// Take the pending NMI of this LAPIC: `true` when one was waiting (S6-12b). The owning vCPU
    /// calls it only after the NMI was delivered through vector 2.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn take_nmi(&self) -> bool {
        self.with_state(|s| std::mem::take(&mut s.pending_nmi))
    }
    /// Test helper: vCPU `sender` writes `destination` to ICR_HIGH and then `low` to ICR_LOW.
    #[cfg(test)]
    pub(crate) fn write_icr(
        &self,
        sender: usize,
        destination: u8,
        low: u32,
    ) -> Result<(), DeviceError> {
        let mut device = LocalApic(self.0.clone());
        let access = |offset| DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width: AccessWidth::Dword,
        };
        device.write_from_cpu(sender, access(0x310), u64::from(destination) << 24)?;
        device.write_from_cpu(sender, access(0x300), u64::from(low))
    }
    /// The state of every Local APIC of the board and of the IOAPIC route as a value (S6-24):
    /// registers, IRR/ISR/TMR, pending INIT/SIPI/NMI, the timers with their event queues
    /// (deadlines in virtual time and the generations) and the Remote IRR of the route.
    pub(crate) fn snapshot_pair(&self) -> PairState {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// Put back a state taken by [`Self::snapshot_pair`] from this same board. A state of another
    /// number of Local APICs, or whose timers are another set (present or absent) or hang on another
    /// clock, is refused and nothing changes. Not a reset: nothing is vetoed by a Remote IRR.
    pub(crate) fn restore_pair(&self, state: &PairState) -> Result<(), DeviceError> {
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let same_timers = pair.cpus.len() == state.cpus.len()
            && pair.cpus.iter().zip(&state.cpus).all(|(now, then)| {
                match (&now.timer, &then.timer) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.clock == b.clock && a.hz == b.hz,
                    _ => false,
                }
            });
        if !same_timers {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        // The wake sink and the COM2 line are properties of this board, not of the snapshot: a
        // snapshot taken on a board without the agent channel does not disconnect INTIN3 here, and
        // one taken with it does not connect INTIN3 on a board that has no COM2.
        let (wake, com2_wired) = (pair.wake.clone(), pair.com2_wired);
        *pair = state.clone();
        pair.wake = wake;
        pair.com2_wired = com2_wired;
        Ok(())
    }
    /// Install (or with `None` remove) the wake sink of the vCPU threads (S6-34, see
    /// [`crate::apic_pair::WakeSink`]): called with the index of the Local APIC every time a
    /// request is posted to it, under the APIC lock.
    pub(crate) fn set_wake_sink(&self, sink: Option<std::sync::Arc<dyn Fn(usize) + Send + Sync>>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).wake = crate::apic_pair::WakeSink(sink);
    }
    /// The power-on reset of every Local APIC and of the IOAPIC route (S6-24, see
    /// [`PairState::reset_all`]); the board reset uses it. [`Self::reset_checked`] and
    /// [`Self::reset_pair`] stay what they were: the reset of one LAPIC and the route.
    pub(crate) fn reset_all(&self) -> Result<(), DeviceError> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).reset_all()
    }
    /// Reset only the Local APIC; a level request awaiting EOI vetoes reset.
    pub fn reset(&self) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset_local(self.1)
    }
    /// Atomically reset the IOAPIC route and the LAPIC of this handle's vCPU, discarding the
    /// requests accepted by them. With several vCPUs the other LAPICs keep the requests already
    /// delivered to them.
    pub fn reset_pair(&self) -> Result<(), DeviceError> {
        self.reset_checked()
    }
    /// Reset this vCPU's LAPIC and the IOAPIC routes, retaining the timer clock and
    /// configuration. The LAPICs of other vCPUs are untouched.
    pub fn reset_checked(&self) -> Result<(), DeviceError> {
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let apic = &mut pair.cpus[self.1];
        if let Some(timer) = &mut apic.timer {
            timer.reset()?;
        }
        apic.local = State::default();
        pair.route = Default::default();
        Ok(())
    }
    /// Advance only a timer configured with its private clock.
    pub fn advance_timer_ns(&self, delta: u64) -> Result<(), DeviceError> {
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let timer = pair.cpus[self.1]
            .timer
            .as_mut()
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        if !timer.can_advance_clock {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        timer
            .clock
            .advance_standalone(delta)
            .map_err(|_| DeviceError::UnsupportedConfiguration)
    }
    /// Dispatch at most one due timer event without entering the CPU.
    pub fn dispatch_timer(&self) -> Result<bool, DeviceError> {
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let timer = pair.cpus[self.1]
            .timer
            .as_ref()
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        let clock = timer.clock.clone();
        let gated = !timer.can_advance_clock;
        let generation = timer.queue.generation();
        let lvt = timer.lvt;
        let mut queue = timer.queue.clone();
        clock.with_dispatch_guard(|clock, dispatch_allowed| {
            let now = clock.now_ns();
            if queue
                .peek_due(generation, now)
                .map_err(|_| DeviceError::UnsupportedConfiguration)?
                .is_none()
            {
                return Ok(false);
            }
            if gated && !dispatch_allowed {
                return Err(DeviceError::Busy);
            }
            if lvt & PERIODIC != 0 {
                let deadline = pair.cpus[self.1]
                    .timer
                    .as_ref()
                    .unwrap()
                    .next_periodic_deadline(now)?;
                queue
                    .replace_single(generation, Some(deadline))
                    .map_err(|_| DeviceError::UnsupportedConfiguration)?;
            } else {
                queue
                    .remove_due(generation, now)
                    .map_err(|_| DeviceError::UnsupportedConfiguration)?;
            }
            if lvt & MASK == 0 {
                pair.edge(self.1, lvt as u8)?;
            }
            // Queue consumption and IRR publication share both locks.
            pair.cpus[self.1].timer.as_mut().unwrap().queue = queue;
            Ok(true)
        })
    }
    /// S7-32: the TPR (offset 0x80), read without any other effect.
    pub(crate) fn tpr(&self) -> u8 {
        self.with_state(|state| state.tpr)
    }

    /// S7-32: the TPR written by `MOV CR8, r64` (TPR = CR8 << 4, TPR[3:0] cleared; SDM Vol. 3A
    /// 11.8.6.5). The processor priority follows from it as for an MMIO write.
    pub(crate) fn set_tpr_from_cr8(&self, cr8: u8) {
        self.with_state(|state| state.tpr = (cr8 & 0xf) << 4);
    }

    /// How many guest writes to the timer registers (`0x320`, `0x380`, `0x3E0`) this LAPIC has
    /// accepted (S6-16d). Read-only; a refused write does not count.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn timer_register_writes(&self) -> u64 {
        self.with_state_pair(|cpu| cpu.timer_writes)
    }
    /// Whether this LAPIC has an opt-in timer (S4-27i2). Read-only.
    pub fn has_timer(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).cpus[self.1]
            .timer
            .is_some()
    }
    /// Virtual-time deadline (ns) of the next timer expiry that can raise an
    /// interrupt (S4-27i2). `None` when there is no timer, no armed event (not
    /// started, cancelled, or a one-shot that already fired) or the LVT Timer is
    /// masked (a masked expiry only re-arms and never raises an IRQ). Read-only:
    /// it consumes no event and does not read or advance the clock.
    pub fn next_timer_deadline(&self) -> Option<u64> {
        let pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let timer = pair.cpus[self.1].timer.as_ref()?;
        if timer.lvt & MASK != 0 {
            return None;
        }
        timer.queue.next_deadline()
    }
    /// Vector of the LVT Timer when a timer exists and is unmasked (S4-27i3).
    /// Read-only; `None` when the timer is absent or masked.
    pub fn timer_vector(&self) -> Option<u8> {
        let pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let timer = pair.cpus[self.1].timer.as_ref()?;
        (timer.lvt & MASK == 0).then_some(timer.lvt as u8)
    }
    /// Disable is rejected while a level request awaits EOI.
    pub fn set_global_enabled(&self, enabled: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_enabled(self.1, enabled)
    }
    /// Record a fixed edge. Repeated requests coalesce in the IRR bit.
    pub fn request_fixed_edge(&self, vector: u8) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .edge(self.1, vector)
    }

    /// Accept an edge MSI for a physical xAPIC ID in this machine or, with `logical` (S7-32, the
    /// DM bit of the message address), a logical destination address resolved through the
    /// LDR/DFR of the vCPUs (the first match: one target, as for lowest priority).
    pub(crate) fn request_msi_edge(
        &self,
        destination: u8,
        logical: bool,
        vector: u8,
    ) -> Result<(), DeviceError> {
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let cpu = pair
            .resolve_destination(destination, logical)
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        pair.edge(cpu, vector)
    }
    /// Observe the highest pending vector without consuming it.
    pub fn pending(&self) -> Option<u8> {
        self.with_state(|s| State::highest(&s.irr))
    }
    /// Observe the IRR and ISR bits of one vector without side effects.
    pub fn vector_state(&self, vector: u8) -> (bool, bool) {
        self.with_state(|s| {
            let index = usize::from(vector / 32);
            let bit = 1 << (vector % 32);
            (s.irr[index] & bit != 0, s.isr[index] & bit != 0)
        })
    }
    /// S4-38L8f: whether `vector`, once requested, could be accepted: the Local APIC is enabled
    /// (global and SVR software enable) and the priority class of `vector` is above the processor
    /// priority (TPR and ISR). A question that changes nothing; IRR is not looked at.
    pub(crate) fn would_accept(&self, vector: u8) -> bool {
        self.with_state(|s| {
            s.global_enabled && s.svr & 0x100 != 0 && (vector & 0xf0) > (s.ppr() & 0xf0)
        })
    }
    /// Observe the highest eligible vector without consuming it.
    pub fn eligible(&self) -> Option<u8> {
        self.with_state(|s| s.eligible())
    }
    /// Move one eligible IRR bit to ISR. This does not execute a guest handler.
    pub fn acknowledge(&self) -> Option<u8> {
        self.with_state(|s| {
            let v = s.eligible()?;
            s.move_to_service(v);
            Some(v)
        })
    }
    /// Spurious vector `SVR[7:0]`, or `None` while the Local APIC is globally
    /// disabled.
    pub fn spurious_vector(&self) -> Option<u8> {
        self.with_state(|s| s.global_enabled.then_some(s.svr as u8))
    }
    /// Acknowledge cycle that resolves at acknowledge time, not at an earlier
    /// selection: the highest eligible request now moves to ISR; when none is
    /// eligible (withdrawn, masked by a raised TPR, software-disabled) the
    /// spurious vector is returned and no request state changes. `None` while
    /// the Local APIC is globally disabled. A caller that must enter one
    /// previously selected vector uses `acknowledge_vector`, which refuses a
    /// changed selection instead of substituting another vector.
    pub fn acknowledge_or_spurious(&self) -> Option<InterruptAcknowledge> {
        self.with_state(|s| {
            if !s.global_enabled {
                return None;
            }
            Some(match s.eligible() {
                Some(v) => {
                    s.move_to_service(v);
                    InterruptAcknowledge::Fixed(v)
                }
                None => InterruptAcknowledge::Spurious(s.svr as u8),
            })
        })
    }

    /// Commit the selected request and CPU entry together. The caller must
    /// serialize all APIC operations across selection and delivery.
    pub(crate) fn acknowledge_vector(&self, vector: u8, publish: impl FnOnce()) -> bool {
        self.with_state(|s| {
            if s.eligible() != Some(vector) {
                return false;
            }
            s.move_to_service(vector);
            publish();
            true
        })
    }

    /// Complete an interrupt entry that the owning vCPU has already committed.
    /// A concurrent sender can queue a higher vector but cannot replace the frame
    /// on the target's stack. Leave that new request in IRR for the next boundary.
    pub(crate) fn acknowledge_delivered_vector(&self, vector: u8) -> bool {
        self.with_state(|s| {
            let index = usize::from(vector / 32);
            let bit = 1 << (vector % 32);
            if s.irr[index] & bit == 0 {
                return false;
            }
            s.move_to_service(vector);
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
        pair.set_enabled(apic.1, effect.new == od_isa::msr::ApicBaseCpu::ENABLED)?;
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

/// ICR low dword fields (SDM 13.6.1).
const ICR_DELIVERY_STATUS: u32 = 1 << 12;
/// Reserved bits 13, 17:16 and 31:20.
const ICR_RESERVED: u64 = 0xfff3_2000;
const ICR_SHORTHAND_NONE: u64 = 0;
const ICR_SHORTHAND_SELF: u64 = 1;
const ICR_SHORTHAND_ALL: u64 = 2;
const ICR_SHORTHAND_OTHERS: u64 = 3;

/// A command of the closed set of ICR commands of SMP_CONTRACT.md 5, decoded and validated.
#[derive(Debug, Clone, Eq, PartialEq)]
enum IpiCommand {
    /// A Fixed edge request in the IRR of every LAPIC in `targets`.
    Fixed { targets: Vec<usize>, vector: u8 },
    /// INIT assert: queue an INIT in the LAPIC of `target` (S6-13).
    InitAssert { target: usize },
    /// INIT deassert: accepted, no change to any CPU or LAPIC state.
    InitDeassert,
    /// SIPI: queue the start-up vector in the LAPIC of `target` (S6-13).
    Startup { target: usize, vector: u8 },
    /// NMI: remember one NMI in the LAPIC of every target (S6-12b).
    Nmi { targets: Vec<usize> },
}

/// The closed set of ICR commands of SMP_CONTRACT.md 5 that this model accepts: Fixed (LAPIC
/// indices to raise an edge in, and the vector), NMI (S6-12b: LAPIC indices to latch an NMI in),
/// INIT assert/deassert and SIPI (S6-13). S7-32: Fixed to a logical destination (every vCPU whose
/// LDR the destination names under its DFR model) and Lowest Priority without shorthand (one
/// target: the physical ID, or the first vCPU the logical destination names), both with Level
/// (bit 14) set or clear and the edge trigger. Everything else is a device error, decided before
/// any state changes: reserved bits, NMI Self and All and the delivery modes the matrix refuses
/// (SMI, the reserved 011 and 111, Lowest Priority with a shorthand), Fixed vectors below 16, INIT
/// assert addressed to the boot processor (deferred to S6-15/S6-16), INIT with a non-zero
/// vector, INIT and SIPI with a shorthand or a logical destination, a logical destination that
/// names no vCPU, a destination APIC ID no vCPU has (this includes the xAPIC broadcast ID 0xFF,
/// which the matrix does not list) and any `(Level, Trigger)` pair the row does not name.
/// Shorthand ignores the destination byte and Destination Mode.
fn decode_icr(pair: &PairState, sender: usize, value: u64) -> Result<IpiCommand, DeviceError> {
    let refused = DeviceError::UnsupportedConfiguration;
    let count = pair.cpus.len();
    if value > u64::from(u32::MAX) || value & ICR_RESERVED != 0 {
        return Err(refused);
    }
    let mode = (value >> 8) & 7;
    let vector = value as u8;
    let level_trigger = (value >> 14) & 3;
    let shorthand = (value >> 18) & 3;
    let physical_target = || {
        let apic_id = usize::from((pair.cpus[sender].local.icr_high >> 24) as u8);
        (apic_id < count).then_some(apic_id).ok_or(refused.clone())
    };
    let destination = (pair.cpus[sender].local.icr_high >> 24) as u8;
    let logical = value & (1 << 11) != 0;
    match mode {
        // Fixed.
        0 => {
            if vector < 16 {
                return Err(refused);
            }
            let targets = match shorthand {
                ICR_SHORTHAND_NONE if !logical && level_trigger == 0b00 => {
                    vec![physical_target()?]
                }
                // S7-32: a logical destination reaches every vCPU it names (SDM Vol. 3A 13.6.2.2);
                // Level (bit 14) is accepted set or clear (it must be 1 for non-INIT IPIs on the
                // P6 family and is ignored on later processors, SDM Vol. 3A 13.6.1), the trigger
                // stays edge.
                ICR_SHORTHAND_NONE if logical && matches!(level_trigger, 0b00 | 0b01) => {
                    let targets = pair.logical_targets(destination);
                    if targets.is_empty() {
                        return Err(refused);
                    }
                    targets
                }
                ICR_SHORTHAND_SELF if matches!(level_trigger, 0b00 | 0b01) => vec![sender],
                ICR_SHORTHAND_ALL if level_trigger == 0b00 => (0..count).collect(),
                ICR_SHORTHAND_OTHERS if level_trigger == 0b00 => {
                    (0..count).filter(|&cpu| cpu != sender).collect()
                }
                _ => return Err(refused),
            };
            Ok(IpiCommand::Fixed { targets, vector })
        }
        // S7-32: lowest priority (001) without shorthand, physical or logical: one target, the
        // first vCPU the destination names (no arbitration among several is modelled). Level
        // (bit 14) set or clear, edge trigger, as for the logical fixed form.
        1 if shorthand == ICR_SHORTHAND_NONE
            && matches!(level_trigger, 0b00 | 0b01)
            && vector >= 16 =>
        {
            let target = pair
                .resolve_destination(destination, logical)
                .ok_or(refused)?;
            Ok(IpiCommand::Fixed {
                targets: vec![target],
                vector,
            })
        }
        // NMI (100): physical destination without shorthand, or all-but-self; the vector field is
        // ignored. Self and All are refused (SMP_CONTRACT.md 5).
        4 => {
            let targets = match shorthand {
                ICR_SHORTHAND_NONE if value & (1 << 11) == 0 && level_trigger == 0b00 => {
                    vec![physical_target()?]
                }
                ICR_SHORTHAND_OTHERS if level_trigger == 0b00 => {
                    (0..count).filter(|&cpu| cpu != sender).collect()
                }
                _ => return Err(refused),
            };
            Ok(IpiCommand::Nmi { targets })
        }
        // INIT (101) and Start-up (110): physical destination, no shorthand.
        5 | 6 => {
            if shorthand != ICR_SHORTHAND_NONE || value & (1 << 11) != 0 {
                return Err(refused);
            }
            match (mode, level_trigger, vector) {
                (5, 0b11, 0) => {
                    let target = physical_target()?;
                    // BSP INIT is deferred to S6-15/S6-16: nothing could apply it yet, so an
                    // INIT assert addressed to the boot processor is refused, before any state
                    // changes, instead of staying pending for ever.
                    if target == BOOT_VCPU_INDEX {
                        return Err(refused);
                    }
                    Ok(IpiCommand::InitAssert { target })
                }
                (5, 0b10, 0) => {
                    physical_target()?;
                    Ok(IpiCommand::InitDeassert)
                }
                (6, 0b00, _) => Ok(IpiCommand::Startup {
                    target: physical_target()?,
                    vector,
                }),
                _ => Err(refused),
            }
        }
        _ => Err(refused),
    }
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
/// Whether `value` is a valid write of the storage-only LVT entry at `offset` (PMC `0x340`,
/// LINT0/LINT1 `0x350`/`0x360`, Error `0x370`): only the entry's bits and, for an entry with a
/// delivery mode, a mode the entry supports (SDM Vol. 3A 13.5.1: `110` is reserved for every
/// entry, `001` and `011` are not encodings, INIT `101` and ExtINT `111` are not supported for
/// the PMC entry).
fn storage_lvt_write_valid(offset: u64, value: u64) -> bool {
    let (bits, modes): (u32, &[u64]) = match offset {
        0x340 => (PMC_LVT_BITS, &[0b000, 0b010, 0b100]),
        0x350 | 0x360 => (LINT_BITS, &[0b000, 0b010, 0b100, 0b101, 0b111]),
        _ => (ERROR_LVT_BITS, &[0b000]),
    };
    value & !u64::from(bits) == 0 && modes.contains(&((value >> 8) & 7))
}
impl BusDevice for LocalApic {
    fn read(&mut self, a: DeviceAccess) -> Result<u64, DeviceError> {
        self.read_from_cpu(BOOT_VCPU_INDEX, a)
    }
    fn write(&mut self, a: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.write_from_cpu(BOOT_VCPU_INDEX, a, value)
    }
    /// The window of the LAPIC of vCPU `cpu`; there is no such LAPIC when the board has fewer
    /// vCPUs, which is `UnsupportedConfiguration`.
    fn read_from_cpu(&mut self, cpu: usize, a: DeviceAccess) -> Result<u64, DeviceError> {
        let offset = validate(a)?;
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let apic = pair
            .cpus
            .get_mut(cpu)
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        if matches!(offset, 0x320 | 0x380 | 0x390 | 0x3e0) {
            if !apic.local.global_enabled {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            let t = apic.timer.as_ref().ok_or(DeviceError::UnknownOffset)?;
            return Ok(u64::from(match offset {
                0x320 => t.lvt,
                0x380 => t.initial,
                0x390 => t.current(),
                _ => t.divide,
            }));
        }
        {
            let s = &mut apic.local;
            if !s.global_enabled {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            let value = match offset {
                // The xAPIC ID register holds the APIC ID in bits 31:24; APIC ID = vCPU index.
                0x20 => (cpu as u32) << 24,
                0x30 => VERSION,
                0xd0 => s.ldr,
                0xe0 => s.dfr,
                0x280 => s.esr,
                0x340 => s.pmc_lvt,
                0x350 => s.lint0,
                0x360 => s.lint1,
                0x370 => s.error_lvt,
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
        }
    }
    fn write_from_cpu(
        &mut self,
        cpu: usize,
        a: DeviceAccess,
        value: u64,
    ) -> Result<(), DeviceError> {
        let offset = validate(a)?;
        let mut pair = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if cpu >= pair.cpus.len() {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        {
            if !pair.cpus[cpu].local.global_enabled {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            if matches!(offset, 0x320 | 0x380 | 0x390 | 0x3e0) {
                let timer = pair.cpus[cpu]
                    .timer
                    .as_ref()
                    .ok_or(DeviceError::UnknownOffset)?;
                if offset == 0x390 {
                    return Err(DeviceError::ReadOnly);
                }
                let now_ns = timer.clock.snapshot().now_ns();
                if timer.due_at(now_ns) {
                    return Err(DeviceError::Busy);
                }
                match offset {
                    0x320 => {
                        // A periodic timer can finish its current countdown as a one-shot.
                        // Entering periodic mode still requires an idle timer.
                        // A masked LVT is accepted even when its vector is below 16. The SDM
                        // permits an APIC to report that illegal vector through ESR regardless
                        // of the mask bit; that optional ESR source is outside this model.
                        if value & !u64::from(MASK | PERIODIC | 0xff) != 0
                            || (value & u64::from(MASK) == 0 && value & 0xff < 16)
                            || (value & u64::from(PERIODIC) != 0
                                && timer.lvt & PERIODIC == 0
                                && !timer.queue.is_empty())
                            || (value & u64::from(MASK) == 0
                                && pair.cpus[cpu].local.svr & 0x100 == 0)
                            || (value & u64::from(MASK) == 0
                                && pair.level_vector_reserved(cpu, value as u8))
                        {
                            return Err(DeviceError::UnsupportedConfiguration);
                        }
                        pair.cpus[cpu].timer.as_mut().unwrap().lvt = value as u32;
                        pair.cpus[cpu].timer_writes += 1;
                        return Ok(());
                    }
                    0x380 if value <= u64::from(u32::MAX) => {
                        let armed = pair.cpus[cpu]
                            .timer
                            .as_mut()
                            .unwrap()
                            .arm_at(now_ns, value as u32);
                        if armed.is_ok() {
                            pair.cpus[cpu].timer_writes += 1;
                        }
                        return armed;
                    }
                    0x3e0
                        if value & !0xb == 0
                            && (timer.queue.is_empty() || value == u64::from(timer.divide)) =>
                    {
                        pair.cpus[cpu].timer.as_mut().unwrap().divide = value as u32;
                        pair.cpus[cpu].timer_writes += 1;
                        return Ok(());
                    }
                    _ => return Err(DeviceError::UnsupportedConfiguration),
                }
            }
            match offset {
                0x80 if value <= 0xff => {
                    pair.cpus[cpu].local.tpr = value as u8;
                    Ok(())
                }
                // S7-32: bit 9 (focus processor checking disable) is stored; with one target per
                // message it changes nothing. Bit 12 (EOI broadcast suppression) stays refused:
                // VERSION does not advertise it.
                0xf0 if value & !0x3ff == 0 => {
                    if value & 0x100 == 0
                        && (pair.awaits_eoi(cpu)
                            || pair.cpus[cpu]
                                .timer
                                .as_ref()
                                .is_some_and(|t| !t.queue.is_empty()))
                    {
                        return Err(DeviceError::UnsupportedConfiguration);
                    }
                    if value & 0x100 == 0 {
                        if let Some(timer) = pair.cpus[cpu].timer.as_mut() {
                            timer.lvt |= MASK;
                        }
                        // Software disable masks every LVT entry (SDM 13.4.7.2).
                        pair.cpus[cpu].local.lint0 |= MASK;
                        pair.cpus[cpu].local.lint1 |= MASK;
                        pair.cpus[cpu].local.error_lvt |= MASK;
                        pair.cpus[cpu].local.pmc_lvt |= MASK;
                    }
                    pair.cpus[cpu].local.svr = value as u16;
                    Ok(())
                }
                0xb0 if value == 0 => pair.eoi(cpu),
                // Interprocessor interrupt by the matrix of SMP_CONTRACT.md 5 (S6-12, S6-13).
                // Delivery completes here: a Fixed request is in the IRR of every target, an INIT
                // or SIPI is pending in the LAPIC of its target. Delivery Status (bit 12) is
                // device-owned, always Idle, and dropped from the stored command. A refused
                // command changes nothing.
                0x300 => {
                    match decode_icr(&pair, cpu, value)? {
                        IpiCommand::Fixed { targets, vector } => pair.edge_all(&targets, vector)?,
                        IpiCommand::InitAssert { target } => pair.queue_init(target)?,
                        IpiCommand::InitDeassert => {}
                        IpiCommand::Startup { target, vector } => {
                            pair.queue_sipi(target, vector)?
                        }
                        IpiCommand::Nmi { targets } => pair.queue_nmi(&targets)?,
                    }
                    pair.cpus[cpu].local.icr_low = value as u32 & !ICR_DELIVERY_STATUS;
                    Ok(())
                }
                0x310 if value & !0xff00_0000 == 0 => {
                    pair.cpus[cpu].local.icr_high = value as u32;
                    Ok(())
                }
                // PMC/LINT0/LINT1/Error LVT (S4-25f, S7-32 slice M): storage and mask only.
                // Unmasking is refused while software disabled, like the Timer LVT.
                0x340 | 0x350 | 0x360 | 0x370
                    if storage_lvt_write_valid(offset, value)
                        && (value & u64::from(MASK) != 0
                            || pair.cpus[cpu].local.svr & 0x100 != 0) =>
                {
                    let stored = value as u32 & !LVT_READ_ONLY_BITS;
                    match offset {
                        0x340 => pair.cpus[cpu].local.pmc_lvt = stored,
                        0x350 => pair.cpus[cpu].local.lint0 = stored,
                        0x360 => pair.cpus[cpu].local.lint1 = stored,
                        _ => pair.cpus[cpu].local.error_lvt = stored,
                    }
                    Ok(())
                }
                // Logical destination routing is not modelled. DFR keeps only
                // the model nibble (flat 0xf, cluster 0x0); bits 27:0 read 1.
                0xe0 if value <= u64::from(u32::MAX) && matches!(value >> 28, 0 | 0xf) => {
                    pair.cpus[cpu].local.dfr = value as u32 | 0x0fff_ffff;
                    Ok(())
                }
                0xd0 if value & !0xff00_0000 == 0 => {
                    pair.cpus[cpu].local.ldr = value as u32;
                    Ok(())
                }
                // The written value is ignored: the write latches the errors
                // accumulated since the last write and clears the log.
                0x280 if value <= u64::from(u32::MAX) => {
                    pair.cpus[cpu].local.esr =
                        std::mem::take(&mut pair.cpus[cpu].local.esr_pending);
                    Ok(())
                }
                0x80 | 0xb0 | 0xf0 | 0x310 | 0x340 | 0x350 | 0x360 | 0x370 | 0xd0 | 0xe0
                | 0x280 => Err(DeviceError::UnsupportedConfiguration),
                0x20 | 0x30 | 0xa0 => Err(DeviceError::ReadOnly),
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
    use crate::apic_pair::LEVEL;
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

    fn setup_device(device: LocalApic) -> (MachineBus, LocalApicHandle) {
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
    fn setup() -> (MachineBus, LocalApicHandle) {
        setup_device(LocalApic::new())
    }
    fn timer_setup(hz: u64) -> (MachineBus, LocalApicHandle) {
        let (mut bus, handle) =
            setup_device(LocalApic::with_timer(crate::VirtualClock::new(1).unwrap(), hz).unwrap());
        write(&mut bus, 0xf0, 0x1ff).unwrap();
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
    fn timer_unmask_while_software_disabled_is_atomic() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, u64::from(MASK | 0x40)).unwrap();
        write(&mut bus, 0xf0, 0xff).unwrap();
        let before = apic.0.lock().unwrap().clone();
        assert_eq!(
            write(&mut bus, 0x320, 0x40),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(*apic.0.lock().unwrap(), before);
    }

    #[test]
    fn timer_software_disable_masks_idle_lvt_through_expiry_and_reenable() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, 0x40).unwrap();
        write(&mut bus, 0xf0, 0xff).unwrap();
        assert_eq!(read(&mut bus, 0x320), Ok(u64::from(MASK | 0x40)));
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x380, 1).unwrap();
        apic.advance_timer_ns(1).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), None);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        assert_eq!(read(&mut bus, 0x320), Ok(u64::from(MASK | 0x40)));
        assert_eq!(apic.pending(), None);
        assert_eq!(apic.eligible(), None);
    }

    #[test]
    fn timer_lvt_rejects_low_unmasked_vectors_and_accepts_masked_vectors() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        for vector in 0..16 {
            let before = apic.0.lock().unwrap().clone();
            assert_eq!(
                write(&mut bus, 0x320, u64::from(vector)),
                Err(error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(*apic.0.lock().unwrap(), before);

            let masked = u64::from(MASK | vector);
            write(&mut bus, 0x320, masked).unwrap();
            assert_eq!(read(&mut bus, 0x320), Ok(masked));
        }
        for vector in [16, 255] {
            for mask in [0, MASK] {
                let value = u64::from(mask | vector);
                write(&mut bus, 0x320, value).unwrap();
                assert_eq!(read(&mut bus, 0x320), Ok(value));
            }
        }
    }

    #[test]
    fn masked_timer_vector_zero_consumes_expiry_without_delivery_and_cannot_be_unmasked() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, u64::from(MASK)).unwrap();
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x380, 2).unwrap();
        apic.advance_timer_ns(2).unwrap();

        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x390), Ok(0));

        let before = snapshot(&apic);
        assert_eq!(
            write(&mut bus, 0x320, 0),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(snapshot(&apic), before);
    }

    #[test]
    fn timer_count_boundary_rearm_cancel_and_dispatch() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, 0x40).unwrap();
        write(&mut bus, 0x3e0, 0xb).unwrap(); // divide by one
        write(&mut bus, 0x380, 3).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(3));
        apic.advance_timer_ns(2).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(1));
        assert!(!apic.dispatch_timer().unwrap());
        write(&mut bus, 0x380, 5).unwrap();
        apic.advance_timer_ns(3).unwrap();
        assert!(!apic.dispatch_timer().unwrap());
        write(&mut bus, 0x380, 0).unwrap();
        apic.advance_timer_ns(3).unwrap();
        assert!(!apic.dispatch_timer().unwrap());
        write(&mut bus, 0x380, 2).unwrap();
        apic.advance_timer_ns(2).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(0));
        assert_eq!(write(&mut bus, 0x320, 0x41), Err(error(DeviceError::Busy)));
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x40));
        assert!(!apic.dispatch_timer().unwrap());
    }

    #[test]
    fn periodic_timer_tracks_fractional_phase_and_coalesces_delay() {
        let (mut bus, apic) = timer_setup(3_000_000_000);
        write(&mut bus, 0x320, u64::from(PERIODIC | 0x40)).unwrap();
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x380, 4).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(4));
        apic.advance_timer_ns(1).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(1));
        assert!(!apic.dispatch_timer().unwrap());
        apic.advance_timer_ns(1).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(2));
        assert_eq!(write(&mut bus, 0x380, 0), Err(error(DeviceError::Busy)));
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.acknowledge(), Some(0x40));
        write(&mut bus, 0xb0, 0).unwrap();
        assert!(!apic.dispatch_timer().unwrap());
        apic.advance_timer_ns(1).unwrap(); // next boundary is 3ns, not 4ns
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.acknowledge(), Some(0x40));
        write(&mut bus, 0xb0, 0).unwrap();
        apic.advance_timer_ns(100).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert!(!apic.dispatch_timer().unwrap());
        assert_eq!(apic.acknowledge(), Some(0x40));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.pending(), None);
        apic.advance_timer_ns(1).unwrap();
        assert!(apic.dispatch_timer().unwrap()); // 104ns is on the original phase
    }

    #[test]
    fn one_shot_preserves_the_in_flight_periodic_countdown() {
        for (lvt, expected_irq) in [(MASK, None), (0xec, Some(0xec))] {
            let (mut bus, apic) = timer_setup(600_000_000);
            write(&mut bus, 0x320, u64::from(PERIODIC | 0x40)).unwrap();
            write(&mut bus, 0x3e0, 3).unwrap(); // divide by 16, as in Linux setup
            write(&mut bus, 0x380, 4).unwrap();
            apic.advance_timer_ns(107).unwrap();
            assert!(apic.dispatch_timer().unwrap());
            assert_eq!(apic.pending(), Some(0x40));
            apic.advance_timer_ns(1).unwrap();
            assert_eq!(read(&mut bus, 0x390), Ok(4));
            assert_eq!(apic.next_timer_deadline(), Some(214));

            let mut expected = apic.0.lock().unwrap().clone();
            expected.cpus[0].timer.as_mut().unwrap().lvt = lvt;
            expected.cpus[0].timer_writes += 1;
            write(&mut bus, 0x320, u64::from(lvt))
                .unwrap_or_else(|error| panic!("one-shot write {lvt:#x} failed: {error:?}"));
            assert_eq!(*apic.0.lock().unwrap(), expected);
            assert!(write(&mut bus, 0x320, u64::from(PERIODIC | 0xec)).is_err());
            assert_eq!(*apic.0.lock().unwrap(), expected);
            write(&mut bus, 0x3e0, 3).expect("rewrite the active divide configuration");
            expected.cpus[0].timer_writes += 1;
            assert_eq!(*apic.0.lock().unwrap(), expected);
            assert_eq!(read(&mut bus, 0x390), Ok(4));
            assert_eq!(read(&mut bus, 0x380), Ok(4));
            assert_eq!(apic.acknowledge(), Some(0x40));

            apic.advance_timer_ns(26).unwrap();
            assert_eq!(read(&mut bus, 0x390), Ok(3));
            apic.advance_timer_ns(79).unwrap();
            assert_eq!(read(&mut bus, 0x390), Ok(1));
            assert!(!apic.dispatch_timer().unwrap());
            apic.advance_timer_ns(1).unwrap();
            assert_eq!(read(&mut bus, 0x390), Ok(0));
            assert!(apic.dispatch_timer().unwrap());
            assert_eq!(apic.acknowledge(), expected_irq);
            assert_eq!(apic.vector_state(0x40), (false, true));
            assert_eq!(apic.next_timer_deadline(), None);
            apic.advance_timer_ns(100).unwrap();
            assert_eq!(read(&mut bus, 0x390), Ok(0));
            assert!(!apic.dispatch_timer().unwrap());
        }
    }

    #[test]
    fn periodic_timer_mask_reprogram_cancel_and_reset() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, u64::from(PERIODIC | MASK | 0x40)).unwrap();
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x380, 3).unwrap();
        let before = apic.0.lock().unwrap().clone();
        assert!(
            write(
                &mut bus,
                0x320,
                u64::from(PERIODIC | MASK | 0x40 | (1 << 18))
            )
            .is_err()
        );
        assert!(write(&mut bus, 0x3e0, 0).is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        apic.advance_timer_ns(3).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(3));
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), None);
        write(&mut bus, 0x320, u64::from(PERIODIC | 0x41)).unwrap();
        apic.advance_timer_ns(2).unwrap();
        write(&mut bus, 0x380, 5).unwrap();
        apic.advance_timer_ns(3).unwrap();
        assert!(!apic.dispatch_timer().unwrap());
        write(&mut bus, 0x380, 0).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(0));
        apic.advance_timer_ns(3).unwrap();
        assert!(!apic.dispatch_timer().unwrap());
        write(&mut bus, 0x380, 2).unwrap();
        apic.reset_checked().unwrap();
        assert_eq!(read(&mut bus, 0x320), Ok(u64::from(MASK)));
        assert_eq!(read(&mut bus, 0x390), Ok(0));
        assert!(!apic.dispatch_timer().unwrap());
    }

    #[test]
    fn periodic_timer_rearm_failures_leave_due_event_and_irr_unchanged() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, u64::from(PERIODIC | 0x40)).unwrap();
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x380, 1).unwrap();
        apic.advance_timer_ns(u64::MAX).unwrap();
        let before = apic.0.lock().unwrap().clone();
        assert!(apic.dispatch_timer().is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        assert_eq!(apic.pending(), None);

        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, u64::from(PERIODIC | 0x40)).unwrap();
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x380, 1).unwrap();
        apic.advance_timer_ns(1).unwrap();
        apic.0.lock().unwrap().cpus[0]
            .timer
            .as_mut()
            .unwrap()
            .queue
            .exhaust_generation_for_test();
        let before = apic.0.lock().unwrap().clone();
        assert!(apic.dispatch_timer().is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn timer_divider_mask_reset_and_overflow_are_bounded() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, u64::from(MASK | 0x40)).unwrap();
        write(&mut bus, 0x3e0, 0).unwrap(); // divide by two
        write(&mut bus, 0x380, 2).unwrap();
        apic.advance_timer_ns(3).unwrap();
        assert_eq!(read(&mut bus, 0x390), Ok(1));
        assert!(write(&mut bus, 0x3e0, 0xb).is_err());
        apic.advance_timer_ns(1).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        write(&mut bus, 0x320, 0x40).unwrap();
        assert_eq!(apic.pending(), None);
        write(&mut bus, 0x380, 10).unwrap();
        apic.reset_checked().unwrap();
        assert_eq!(read(&mut bus, 0x320), Ok(u64::from(MASK)));
        assert_eq!(read(&mut bus, 0x390), Ok(0));
        assert_eq!(
            apic.0.lock().unwrap().cpus[0]
                .timer
                .as_ref()
                .unwrap()
                .clock
                .snapshot()
                .now_ns(),
            4
        );
        apic.advance_timer_ns(u64::MAX - 4).unwrap();
        assert!(write(&mut bus, 0x380, 1).is_err());
        assert_eq!(read(&mut bus, 0x380), Ok(0));
    }

    #[test]
    fn timer_generation_and_reset_failure_are_atomic() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x380, 5).unwrap();
        let old = apic.0.lock().unwrap().cpus[0]
            .timer
            .as_ref()
            .unwrap()
            .queue
            .generation();
        write(&mut bus, 0x380, 6).unwrap();
        {
            let mut pair = apic.0.lock().unwrap();
            let timer = pair.cpus[0].timer.as_mut().unwrap();
            assert!(timer.queue.validate_generation(old).is_err());
            timer.queue.exhaust_generation_for_test();
        }
        let before = apic.0.lock().unwrap().clone();
        assert!(apic.reset_checked().is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        assert!(apic.set_global_enabled(false).is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        assert!(write(&mut bus, 0x380, 0).is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        let mut cpu = od_isa::msr::ApicBaseCpu::new(od_isa::LongModeState::default());
        cpu.state.gpr[od_isa::Register::Rcx as usize] = 0x1b;
        cpu.state.gpr[od_isa::Register::Rax as usize] = od_isa::msr::ApicBaseCpu::DISABLED;
        let original = cpu.clone();
        let context = od_isa::AccessContext {
            privilege: od_isa::PrivilegeLevel::Ring0,
            ..Default::default()
        };
        assert!(
            step_apic_base_msr(
                &mut cpu,
                &apic,
                &mut MsrCode(vec![0x0f, 0x30]),
                context,
                1,
                false
            )
            .is_err()
        );
        assert_eq!(cpu, original);
        assert_eq!(*apic.0.lock().unwrap(), before);
    }

    #[test]
    fn timer_rejects_level_vector_conflict_in_both_directions() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, 0x40).unwrap();
        assert!(apic.0.lock().unwrap().set_low(LEVEL | 0x40).is_err());
        write(&mut bus, 0x320, u64::from(MASK | 0x40)).unwrap();
        apic.0.lock().unwrap().set_low(LEVEL | 0x40).unwrap();
        assert!(write(&mut bus, 0x320, 0x40).is_err());
    }

    #[test]
    fn timer_failed_edge_retains_due_event() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x320, 0x40).unwrap();
        write(&mut bus, 0x380, 1).unwrap();
        apic.advance_timer_ns(2).unwrap();
        // Simulate an edge refusal to exercise the transactional retry path.
        apic.0.lock().unwrap().route.low = LEVEL | 0x40;
        let before = apic.0.lock().unwrap().clone();
        assert!(apic.dispatch_timer().is_err());
        assert_eq!(*apic.0.lock().unwrap(), before);
        apic.0.lock().unwrap().route.low = MASK;
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x40));
    }

    #[test]
    fn timer_disable_and_local_reset_keep_clock_and_invalidate_deadline() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x320, 0x40).unwrap();
        write(&mut bus, 0x380, 10).unwrap();
        apic.advance_timer_ns(1).unwrap();
        assert!(write(&mut bus, 0xf0, 0xff).is_err());
        apic.reset().unwrap();
        assert_eq!(read(&mut bus, 0x320), Ok(u64::from(MASK)));
        assert!(!apic.dispatch_timer().unwrap());
        write(&mut bus, 0x380, 10).unwrap();
        apic.set_global_enabled(false).unwrap();
        apic.set_global_enabled(true).unwrap();
        assert_eq!(read(&mut bus, 0x380), Ok(0));
        assert!(!apic.dispatch_timer().unwrap());
        let pair = apic.0.lock().unwrap();
        let timer = pair.cpus[0].timer.as_ref().unwrap();
        assert_eq!(timer.clock.snapshot().now_ns(), 1);
        assert_eq!(timer.hz, 1_000_000_000);
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
            0x4000 | 0x60,                           // physical 0x7f: Level=1 and no such APIC ID
            0x8_4000 | 0x60,                         // All including self: Level=1 only
            SELF_ASSERT | 0x8000 | 0x60,             // level-triggered
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
            // S7-32: bit 9 (focus disable) is stored; bit 12 (EOI broadcast suppression) is not.
            (0xf0, 0x1000, DeviceError::UnsupportedConfiguration),
            (0xb0, 1, DeviceError::UnsupportedConfiguration),
            (0x20, 0, DeviceError::ReadOnly),
            (0x120, 0, DeviceError::ReadOnly),
            (0x1a0, 0, DeviceError::ReadOnly),
            (0x230, 0, DeviceError::ReadOnly),
            (0x30, 0, DeviceError::ReadOnly), // S4-25f: Version is now read-only
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

    // ---- S4-25e: closing slice ------------------------------------------

    const SELF_ASSERT: u64 = 0x4_4000;
    const UC: DeviceError = DeviceError::UnsupportedConfiguration;

    fn snapshot(apic: &LocalApicHandle) -> PairState {
        apic.0.lock().unwrap().clone()
    }

    fn set_vectors(bits: &mut [u32; 8], vectors: &[u8]) {
        *bits = [0; 8];
        for v in vectors {
            bits[usize::from(v / 32)] |= 1 << (v % 32);
        }
    }

    fn isr_vectors(bus: &mut MachineBus) -> Vec<u8> {
        let mut out = vec![];
        for word in 0..8u64 {
            let value = read(bus, 0x100 + word * 16).unwrap();
            for bit in 0..32u64 {
                if value & (1 << bit) != 0 {
                    out.push((word * 32 + bit) as u8);
                }
            }
        }
        out
    }

    /// Leave every Local APIC register, the level route TMR bit and the timer
    /// LVT non-zero. The timer stays idle. Expects `timer_setup`.
    fn dirty(bus: &mut MachineBus, apic: &LocalApicHandle) {
        write(bus, 0xf0, 0x1aa).unwrap();
        {
            let mut pair = apic.0.lock().unwrap();
            pair.set_low(LEVEL | 0x71).unwrap();
            pair.set_line(true).unwrap();
        }
        assert_eq!(apic.acknowledge(), Some(0x71));
        apic.0.lock().unwrap().set_line(false).unwrap();
        write(bus, 0xb0, 0).unwrap(); // TMR bit of 0x71 survives its EOI
        write(bus, 0x300, SELF_ASSERT | 0x51).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        write(bus, 0x300, SELF_ASSERT | 0x61).unwrap();
        write(bus, 0x310, 0xab00_0000).unwrap();
        write(bus, 0x80, 0x3f).unwrap();
        write(bus, 0x320, 0x40).unwrap();
        // S4-25f registers.
        write(bus, 0x350, 0x731).unwrap();
        write(bus, 0x360, 0x8400).unwrap();
        write(bus, 0x370, 0xfe).unwrap();
        // S7-32 slice M: the PMC LVT (NMI, the vector is kept).
        write(bus, 0x340, 0x4fe).unwrap();
        write(bus, 0xe0, 0x0fff_ffff).unwrap();
        write(bus, 0xd0, 0x0100_0000).unwrap();
        apic.0.lock().unwrap().cpus[0].local.esr_pending = 0x40;
        write(bus, 0x280, 0).unwrap();
        apic.0.lock().unwrap().cpus[0].local.esr_pending = 0x80;
        assert_eq!(read(bus, 0x280), Ok(0x40));
        assert_eq!(read(bus, 0x1b0), Ok(1 << 17)); // TMR 0x71
        assert_eq!(read(bus, 0x120), Ok(1 << 17)); // ISR 0x51
        assert_eq!(read(bus, 0x230), Ok(1 << 1)); // IRR 0x61
    }

    fn arm_timer(bus: &mut MachineBus) {
        write(bus, 0x320, u64::from(PERIODIC | 0x40)).unwrap();
        write(bus, 0x3e0, 0xb).unwrap();
        write(bus, 0x380, 5).unwrap();
    }

    fn assert_reset_matrix(bus: &mut MachineBus, apic: &LocalApicHandle, label: &str) {
        for (offset, expected) in [
            (0x20, 0),
            (0x80, 0),
            (0xa0, 0),
            (0xf0, 0xff), // software disabled, spurious vector 0xff
            (0x300, 0),
            (0x310, 0),
            (0x320, u64::from(MASK)), // every supported LVT entry is masked
            (0x380, 0),
            (0x390, 0),
            (0x3e0, 0),
            (0x30, u64::from(VERSION)),
            (0x340, u64::from(MASK)),
            (0x350, u64::from(MASK)),
            (0x360, u64::from(MASK)),
            (0x370, u64::from(MASK)),
            (0x280, 0),
            (0xd0, 0),
            (0xe0, 0xffff_ffff),
        ] {
            assert_eq!(read(bus, offset), Ok(expected), "{label} {offset:#x}");
        }
        for bank in [0x100u64, 0x180, 0x200] {
            for word in 0..8 {
                assert_eq!(
                    read(bus, bank + word * 16),
                    Ok(0),
                    "{label} bank {bank:#x} word {word}"
                );
            }
        }
        assert_eq!(read(bus, 0xb0), Err(error(DeviceError::WriteOnly)));
        assert_eq!(
            snapshot(apic).cpus[0].local.esr_pending,
            0,
            "{label} esr log"
        );
        assert_eq!(apic.pending(), None, "{label}");
        assert_eq!(apic.eligible(), None, "{label}");
        assert_eq!(apic.spurious_vector(), Some(0xff), "{label}");
        apic.advance_timer_ns(100).unwrap();
        assert!(!apic.dispatch_timer().unwrap(), "{label} deadline survived");
    }

    #[test]
    fn reset_matrix_is_identical_for_fresh_device_and_every_reset_path() {
        let (mut bus, apic) = setup_device(
            LocalApic::with_timer(crate::VirtualClock::new(1).unwrap(), 1_000_000_000).unwrap(),
        );
        assert_reset_matrix(&mut bus, &apic, "fresh");

        let paths: [(&str, fn(&LocalApicHandle)); 5] = [
            ("reset", |a| a.reset().unwrap()),
            ("reset_checked", |a| a.reset_checked().unwrap()),
            ("reset_pair", |a| a.reset_pair().unwrap()),
            // The board power-on reset of every Local APIC and the route (S6-24).
            ("reset_all", |a| a.reset_all().unwrap()),
            ("global disable/enable", |a| {
                a.set_global_enabled(false).unwrap();
                a.set_global_enabled(true).unwrap();
            }),
        ];
        for (label, reset) in paths {
            let (mut bus, apic) = timer_setup(1_000_000_000);
            dirty(&mut bus, &apic);
            arm_timer(&mut bus);
            assert_eq!(read(&mut bus, 0xf0), Ok(0x1aa));
            assert_ne!(read(&mut bus, 0x390), Ok(0));
            reset(&apic);
            assert_reset_matrix(&mut bus, &apic, label);
        }
    }

    #[test]
    fn pair_reset_clears_route_and_local_reset_is_vetoed_by_remote_irr() {
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        {
            let mut pair = apic.0.lock().unwrap();
            pair.set_low(LEVEL | 0x51).unwrap();
            pair.set_line(true).unwrap();
        }
        assert_eq!(read(&mut bus, 0x1a0), Ok(1 << 17));
        let before = snapshot(&apic);
        assert_eq!(apic.reset(), Err(UC));
        assert_eq!(snapshot(&apic), before);
        apic.reset_pair().unwrap();
        assert_eq!(read(&mut bus, 0x1a0), Ok(0));
        assert_eq!(read(&mut bus, 0x220), Ok(0));
        let pair = snapshot(&apic);
        assert_eq!(pair.route.low, MASK);
        assert!(!pair.route.remote_irr && !pair.route.high_line);
    }

    #[test]
    fn spurious_vector_is_returned_when_selection_is_withdrawn() {
        use InterruptAcknowledge::{Fixed, Spurious};
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1aa).unwrap();
        assert_eq!(apic.spurious_vector(), Some(0xaa));

        // TPR raised after selection: the stale selection is refused and the
        // acknowledge cycle returns SVR[7:0] without touching any request.
        apic.request_fixed_edge(0x52).unwrap();
        let selected = apic.eligible().unwrap();
        write(&mut bus, 0x80, 0x5f).unwrap();
        let before = snapshot(&apic);
        let mut published = false;
        assert!(!apic.acknowledge_vector(selected, || published = true));
        assert!(!published);
        assert_eq!(snapshot(&apic), before);
        assert_eq!(apic.acknowledge_or_spurious(), Some(Spurious(0xaa)));
        assert_eq!(snapshot(&apic), before);
        assert_eq!(apic.vector_state(0xaa), (false, false));
        write(&mut bus, 0x80, 0).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Fixed(0x52)));
        write(&mut bus, 0xb0, 0).unwrap();

        // A higher vector arrives after selection: the acknowledge cycle
        // resolves the highest eligible vector at that moment, not the stale
        // one; acknowledge_vector refuses to substitute.
        apic.request_fixed_edge(0x52).unwrap();
        let selected = apic.eligible().unwrap();
        apic.request_fixed_edge(0x72).unwrap();
        let before = snapshot(&apic);
        assert!(!apic.acknowledge_vector(selected, || unreachable!()));
        assert_eq!(snapshot(&apic), before);
        assert_eq!(apic.acknowledge_or_spurious(), Some(Fixed(0x72)));
        assert_eq!(apic.vector_state(0x72), (false, true));
        assert_eq!(apic.vector_state(0x52), (true, false));
        // The lower request is now blocked by the in-service class.
        assert_eq!(apic.acknowledge_or_spurious(), Some(Spurious(0xaa)));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Fixed(0x52)));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Spurious(0xaa)));

        // Software disable withholds the request but keeps it in IRR.
        apic.request_fixed_edge(0x52).unwrap();
        write(&mut bus, 0xf0, 0xaa).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Spurious(0xaa)));
        assert_eq!(apic.vector_state(0x52), (true, false));
        write(&mut bus, 0xf0, 0x1f3).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Fixed(0x52)));
        write(&mut bus, 0xb0, 0).unwrap();

        // A spurious cycle never enters ISR, so an in-service vector and PPR
        // stay as they were.
        apic.request_fixed_edge(0x41).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Fixed(0x41)));
        apic.request_fixed_edge(0x42).unwrap(); // same class: blocked
        let before = snapshot(&apic);
        assert_eq!(apic.acknowledge_or_spurious(), Some(Spurious(0xf3)));
        assert_eq!(snapshot(&apic), before);
        assert_eq!(isr_vectors(&mut bus), vec![0x41]);
        assert_eq!(read(&mut bus, 0xa0), Ok(0x40));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Fixed(0x42)));
        write(&mut bus, 0xb0, 0).unwrap();

        // Reset withdraws a pending request; the vector reverts to 0xff.
        apic.request_fixed_edge(0x52).unwrap();
        apic.reset().unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), Some(Spurious(0xff)));
        assert_eq!(apic.pending(), None);

        // Global disable closes the cycle altogether.
        apic.set_global_enabled(false).unwrap();
        assert_eq!(apic.acknowledge_or_spurious(), None);
        assert_eq!(apic.spurious_vector(), None);
        assert_eq!(apic.acknowledge(), None);
    }

    #[test]
    fn masked_timer_lvt_drops_expiry_without_touching_requests_or_replaying() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x3e0, 0xb).unwrap();
        apic.request_fixed_edge(0x40).unwrap();
        apic.request_fixed_edge(0x70).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x70));
        let before = apic.with_state(|s| s.clone());

        // Masked expiry: consumed, no IRR/ISR/TMR change.
        write(&mut bus, 0x320, u64::from(MASK)).unwrap();
        write(&mut bus, 0x380, 2).unwrap();
        apic.advance_timer_ns(2).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.with_state(|s| s.clone()), before);
        assert_eq!(read(&mut bus, 0x390), Ok(0));

        // An illegal low vector cannot be unmasked; refusal preserves the timer and LAPIC.
        assert_eq!(
            write(&mut bus, 0x320, 0),
            Err(error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(apic.with_state(|s| s.clone()), before);

        // Reprogramming a valid vector does not replay the dropped event.
        write(&mut bus, 0x320, 0x44).unwrap();
        assert_eq!(apic.with_state(|s| s.clone()), before);
        apic.advance_timer_ns(1000).unwrap();
        assert!(!apic.dispatch_timer().unwrap());
        assert_eq!(apic.with_state(|s| s.clone()), before);

        // Unmasked expiry sets exactly the LVT vector.
        write(&mut bus, 0x380, 2).unwrap();
        apic.advance_timer_ns(2).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        let after = apic.with_state(|s| s.clone());
        assert_eq!(after.irr[2], before.irr[2] | 1 << 4);
        assert_eq!((after.isr, after.tmr), (before.isr, before.tmr));

        // The mask bit is sampled at expiry, not at arming time.
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x320, 0x44).unwrap();
        write(&mut bus, 0x380, 4).unwrap();
        write(&mut bus, 0x320, u64::from(MASK | 0x44)).unwrap();
        apic.advance_timer_ns(4).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), None);
        write(&mut bus, 0x320, u64::from(MASK | 0x44)).unwrap();
        write(&mut bus, 0x380, 4).unwrap();
        write(&mut bus, 0x320, 0x44).unwrap();
        apic.advance_timer_ns(4).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x44));

        // Periodic: a masked occurrence re-arms and is dropped; the next
        // unmasked one is delivered.
        let (mut bus, apic) = timer_setup(1_000_000_000);
        write(&mut bus, 0x3e0, 0xb).unwrap();
        write(&mut bus, 0x320, u64::from(PERIODIC | MASK | 0x44)).unwrap();
        write(&mut bus, 0x380, 2).unwrap();
        apic.advance_timer_ns(2).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), None);
        write(&mut bus, 0x320, u64::from(PERIODIC | 0x44)).unwrap();
        assert_eq!(apic.pending(), None);
        apic.advance_timer_ns(2).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x44));
    }

    #[test]
    fn lvt_entries_without_a_model_are_unsupported_without_effect() {
        // S4-25f: Version, LINT0/1, Error and ESR were removed from this list; S7-32 slice M: PMC.
        let lvts = [0x2f0, 0x330];
        let (mut bus, timer) = timer_setup(1_000_000_000);
        let (mut plain_bus, plain) = setup();
        for (bus, apic) in [(&mut bus, &timer), (&mut plain_bus, &plain)] {
            let before = snapshot(apic);
            for offset in lvts {
                let unknown = || error(DeviceError::UnknownOffset);
                assert_eq!(read(bus, offset), Err(unknown()));
                for value in [0, u64::from(MASK | 0x40), u64::MAX] {
                    assert_eq!(write(bus, offset, value), Err(unknown()));
                }
            }
            assert_eq!(snapshot(apic), before);
        }
        // Without the opt-in timer even its registers do not exist.
        let before = snapshot(&plain);
        for offset in [0x320, 0x380, 0x390, 0x3e0] {
            let unknown = || error(DeviceError::UnknownOffset);
            assert_eq!(read(&mut plain_bus, offset), Err(unknown()));
            assert_eq!(write(&mut plain_bus, offset, 0), Err(unknown()));
        }
        assert_eq!(snapshot(&plain), before);
    }

    #[test]
    fn eligibility_requires_class_strictly_above_tpr_and_service_class() {
        let (mut bus, apic) = setup();
        // (TPR, ISR vectors, IRR vectors, expected PPR, expected eligible)
        let cases: &[(u8, &[u8], &[u8], u8, Option<u8>)] = &[
            (0x4f, &[], &[0x50], 0x4f, Some(0x50)),
            (0x50, &[], &[0x5f], 0x50, None), // same class as TPR
            (0x50, &[], &[0x60], 0x50, Some(0x60)),
            (0xff, &[], &[0xff], 0xff, None),
            (0xef, &[], &[0xff], 0xef, Some(0xff)),
            (0xf0, &[], &[0xff], 0xf0, None),
            (0x00, &[], &[0x10], 0x00, Some(0x10)),
            (0x00, &[0x51], &[0x5f], 0x50, None), // same class as ISR
            (0x00, &[0x51], &[0x60], 0x50, Some(0x60)),
            (0x70, &[0x51], &[0x65], 0x70, None), // TPR wins over ISR
            (0x30, &[0x80], &[0x7f], 0x80, None), // ISR wins over TPR
            (0x3f, &[0x51], &[0x61], 0x50, Some(0x61)),
            (0x00, &[0x30, 0x52, 0x5f], &[0x5e], 0x50, None), // highest ISR
            (0x00, &[0x41, 0x30], &[0x35], 0x40, None),
            (0x00, &[0x30], &[0x41, 0x35], 0x30, Some(0x41)),
            (0x40, &[], &[0x5f, 0x41], 0x40, Some(0x5f)), // highest IRR vector
            (0x5f, &[], &[0x52, 0x35], 0x5f, None),
        ];
        for (tpr, isr, irr, ppr, eligible) in cases {
            write(&mut bus, 0xf0, 0x1ff).unwrap();
            apic.with_state(|s| {
                s.tpr = *tpr;
                set_vectors(&mut s.isr, isr);
                set_vectors(&mut s.irr, irr);
            });
            let label = format!("tpr {tpr:#x} isr {isr:x?} irr {irr:x?}");
            assert_eq!(read(&mut bus, 0xa0), Ok(u64::from(*ppr)), "{label}");
            assert_eq!(apic.eligible(), *eligible, "{label}");
            assert_eq!(apic.acknowledge(), *eligible, "{label}");
            if let Some(v) = eligible {
                assert_eq!(apic.vector_state(*v), (false, true), "{label}");
            }
            // Software disable withholds everything but keeps the registers.
            apic.with_state(|s| {
                s.tpr = *tpr;
                set_vectors(&mut s.isr, isr);
                set_vectors(&mut s.irr, irr);
            });
            write(&mut bus, 0xf0, 0xff).unwrap();
            assert_eq!(apic.eligible(), None, "{label} soft-disabled");
            assert_eq!(apic.acknowledge(), None, "{label} soft-disabled");
            assert_eq!(read(&mut bus, 0xa0), Ok(u64::from(*ppr)), "{label}");
        }
    }

    #[test]
    fn eoi_clears_only_the_highest_in_service_vector() {
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.with_state(|s| {
            set_vectors(&mut s.isr, &[0x30, 0x41, 0x52, 0x5f]);
            set_vectors(&mut s.irr, &[0x35]);
            set_vectors(&mut s.tmr, &[0x52]);
        });
        let order = [0x5f, 0x52, 0x41, 0x30];
        for (i, _) in order.iter().enumerate() {
            write(&mut bus, 0xb0, 0).unwrap();
            let mut remaining = order[i + 1..].to_vec();
            remaining.sort_unstable();
            assert_eq!(isr_vectors(&mut bus), remaining, "after EOI {i}");
            assert_eq!(apic.vector_state(0x35), (true, false));
            assert_eq!(read(&mut bus, 0x1a0), Ok(1 << 18)); // TMR untouched
            assert_eq!(read(&mut bus, 0x80), Ok(0));
        }
        assert_eq!(apic.pending(), Some(0x35));
        // EOI with an empty ISR is accepted and changes nothing.
        let before = snapshot(&apic);
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(snapshot(&apic), before);

        // Nested real flow: EOI releases the highest class first.
        apic.reset().unwrap();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x41).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x41));
        apic.request_fixed_edge(0x4f).unwrap();
        assert_eq!(apic.eligible(), None); // same class as in-service 0x41
        apic.request_fixed_edge(0x62).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x62));
        assert_eq!(read(&mut bus, 0xa0), Ok(0x60));
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(isr_vectors(&mut bus), vec![0x41]);
        assert_eq!(read(&mut bus, 0xa0), Ok(0x40));
        assert_eq!(apic.eligible(), None);
        write(&mut bus, 0xb0, 0).unwrap();
        assert_eq!(isr_vectors(&mut bus), Vec::<u8>::new());
        assert_eq!(apic.eligible(), Some(0x4f));
    }

    #[test]
    fn erroneous_accesses_are_rejected_without_state_change() {
        use AccessWidth::{Byte, Dword, Qword, Word};
        let (mut bus, apic) = timer_setup(1_000_000_000);
        dirty(&mut bus, &apic);
        let before = snapshot(&apic);
        let mut device = LocalApic(Arc::clone(&apic.0));

        // Wrong space, width or alignment: every offset, read and write.
        for offset in 0..0x1000u64 {
            for width in [Byte, Word, Dword, Qword] {
                for space in [IoSpace::Mmio, IoSpace::Pio] {
                    let expected = if space != IoSpace::Mmio {
                        DeviceError::UnsupportedSpace
                    } else if width != Dword {
                        DeviceError::UnsupportedWidth
                    } else if offset % 16 != 0 {
                        DeviceError::Misaligned
                    } else {
                        continue;
                    };
                    let access = DeviceAccess {
                        space,
                        offset,
                        width,
                    };
                    assert_eq!(device.read(access), Err(expected.clone()));
                    for value in [0, 0x60, u64::MAX] {
                        assert_eq!(device.write(access, value), Err(expected.clone()));
                    }
                }
            }
            assert_eq!(snapshot(&apic), before, "offset {offset:#x}");
        }

        // Aligned dword offsets that are not implemented.
        let supported = |o: u64| {
            matches!(
                o,
                0x20 | 0x30
                    | 0x80
                    | 0xa0
                    | 0xb0
                    | 0xd0
                    | 0xe0
                    | 0xf0
                    | 0x280
                    | 0x300
                    | 0x310
                    | 0x320
                    | 0x340
                    | 0x350
                    | 0x360
                    | 0x370
                    | 0x380
                    | 0x390
                    | 0x3e0
            ) || (0x100..0x280).contains(&o)
        };
        let unknown = DeviceError::UnknownOffset;
        for offset in (0..0x1000u64).step_by(16).filter(|o| !supported(*o)) {
            assert_eq!(read(&mut bus, offset), Err(error(unknown.clone())));
            for value in [0, 0x60, u64::MAX] {
                assert_eq!(write(&mut bus, offset, value), Err(error(unknown.clone())));
            }
        }
        assert_eq!(snapshot(&apic), before);

        // Reserved bits and read-only registers.
        let ro = DeviceError::ReadOnly;
        let mut rejected: Vec<(u64, u64, DeviceError)> = vec![
            (0x80, 0x100, UC),
            (0x80, 0x1_0000_0040, UC),
            (0x80, u64::MAX, UC),
            (0xf0, 0x1000, UC), // bit 9 is accepted since S7-32, bit 12 is not
            (0xf0, 0x400, UC),
            (0xf0, 0x8000_0000, UC),
            (0xf0, 0x1_0000_00ff, UC),
            (0xb0, 1, UC),
            (0xb0, 0x8000_0000, UC),
            (0xb0, 0x1_0000_0000, UC),
            (0x300, 0, UC),
            (0x300, SELF_ASSERT | 0x20_0000 | 0x60, UC),
            (0x300, 0x1_0004_4060, UC),
            (0x310, 1, UC),
            (0x310, 0x0100_0001, UC),
            (0x310, 0x1_0000_0000, UC),
            (0x320, 0x0f, UC),
            (0x320, u64::from(1u32 << 18 | 0x40), UC),
            (0x320, 0x1040, UC), // delivery status is read-only, reserved here
            (0x320, 0x2040, UC),
            (0x320, 0x8040, UC),
            (0x320, 0x1_0000_0040, UC),
            (0x380, 0x1_0000_0000, UC),
            (0x3e0, 0x4, UC),
            (0x3e0, 0x10, UC),
            (0x3e0, 0x1_0000_000b, UC),
            (0x350, 0x800, UC), // reserved bit 11
            (0x350, 0x20_0000, UC),
            (0x350, 0x1_0000_0000, UC),
            (0x350, 0x100, UC), // reserved delivery mode 001
            (0x360, 0x300, UC), // reserved delivery mode 011
            (0x360, 0x600, UC), // reserved delivery mode 110
            (0x370, 0x100, UC), // Error LVT has no delivery mode
            (0x370, 0x2000, UC),
            (0x370, 0x8000, UC),
            (0x370, 0x1_0000_00fe, UC),
            // S7-32 slice M, PMC LVT (SDM Vol. 3A 13.5.1, Figure 13-8): delivery modes 001 and
            // 011 are no encodings, INIT (101) and ExtINT (111) are not supported for this entry
            // even masked, 110 is reserved; bit 11, polarity (13), Remote IRR (14), trigger (15)
            // and bits 31:17 are reserved.
            (0x340, 0x100, UC),
            (0x340, 0x300, UC),
            (0x340, 0x500, UC),
            (0x340, 0x600, UC),
            (0x340, 0x700, UC),
            (0x340, 0x1_0500, UC),
            (0x340, 0x1_0700, UC),
            (0x340, 0x800, UC),
            (0x340, 0x2000, UC),
            (0x340, 0x4000, UC),
            (0x340, 0x8000, UC),
            (0x340, 0x2_0000, UC),
            (0x340, 0x8000_0000, UC),
            (0x340, 0x1_0000_00fe, UC),
            (0xd0, 0x01, UC),
            (0xd0, 0x00ff_ffff, UC),
            (0xd0, 0x1_0000_0000, UC),
            (0xe0, 0x1fff_ffff, UC), // undefined model
            (0xe0, 0x1_ffff_ffff, UC),
            (0x280, 0x1_0000_0000, UC),
            (0x20, 0, ro.clone()),
            (0x30, 0, ro.clone()),
            (0xa0, 0, ro.clone()),
            (0x390, 0, ro.clone()),
        ];
        for bank in [0x100u64, 0x180, 0x200] {
            for word in 0..8 {
                rejected.push((bank + word * 16, 0, ro.clone()));
            }
        }
        for (offset, value, expected) in rejected {
            // Directly on the device: the bus already truncates to the width.
            let access = DeviceAccess {
                space: IoSpace::Mmio,
                offset,
                width: Dword,
            };
            assert_eq!(
                device.write(access, value),
                Err(expected),
                "{offset:#x} <- {value:#x}"
            );
            assert_eq!(snapshot(&apic), before, "{offset:#x} <- {value:#x}");
        }
        assert_eq!(read(&mut bus, 0xb0), Err(error(DeviceError::WriteOnly)));
        assert_eq!(snapshot(&apic), before);

        // Reads of every implemented register are side-effect free.
        for offset in (0..0x1000u64).step_by(16).filter(|o| supported(*o)) {
            let _ = read(&mut bus, offset);
        }
        assert_eq!(snapshot(&apic), before);

        // A global-disabled APIC refuses every implemented register.
        apic.0.lock().unwrap().cpus[0].local.global_enabled = false;
        let disabled = snapshot(&apic);
        for offset in (0..0x1000u64).step_by(16).filter(|o| supported(*o)) {
            assert_eq!(read(&mut bus, offset), Err(error(UC)), "{offset:#x}");
            assert_eq!(write(&mut bus, offset, 0), Err(error(UC)), "{offset:#x}");
        }
        assert_eq!(snapshot(&apic), disabled);
        apic.0.lock().unwrap().cpus[0].local.global_enabled = true;

        // The rejected traffic did not wedge the device.
        write(&mut bus, 0x80, 0x20).unwrap();
        assert_eq!(read(&mut bus, 0x80), Ok(0x20));
    }

    // ---- S4-25f: Linux probe registers ------------------------------------

    /// The storage-only LVT entries: PMC (S7-32 slice M), LINT0, LINT1, Error (S4-25f).
    const LVT_REGS: [u64; 4] = [0x340, 0x350, 0x360, 0x370];

    #[test]
    fn version_reads_integrated_five_lvt_and_is_read_only() {
        let (mut bus, apic) = setup();
        assert_eq!(read(&mut bus, 0x30), Ok(0x0004_0014));
        let before = snapshot(&apic);
        for value in [0, 0x0004_0014, u64::from(u32::MAX)] {
            assert_eq!(
                write(&mut bus, 0x30, value),
                Err(error(DeviceError::ReadOnly))
            );
        }
        assert_eq!(snapshot(&apic), before);
        assert_eq!(read(&mut bus, 0x30), Ok(0x0004_0014));
        // Version bits: 7:0 version, 23:16 Max LVT entry, 24 EOI-broadcast.
        assert_eq!(VERSION & 0xff, 0x14);
        assert_eq!((VERSION >> 16) & 0xff, 4);
        assert_eq!(VERSION >> 24, 0);
        // Readable with or without an opt-in timer.
        let (mut timer_bus, _) = timer_setup(1_000_000_000);
        assert_eq!(read(&mut timer_bus, 0x30), Ok(0x0004_0014));
        // SDM Vol. 3A 13.4.8: Max LVT Entry is the number of LVT entries minus 1. With the timer
        // the device implements exactly that many of the LVT addresses of Table 13-1.
        let implemented = [0x2f0, 0x320, 0x330, 0x340, 0x350, 0x360, 0x370]
            .into_iter()
            .filter(|offset| read(&mut timer_bus, *offset).is_ok())
            .count();
        assert_eq!(implemented as u32, ((VERSION >> 16) & 0xff) + 1);
    }

    #[test]
    fn storage_lvts_reset_masked_and_store_mask_and_vector() {
        let (mut bus, apic) = setup();
        for offset in LVT_REGS {
            assert_eq!(read(&mut bus, offset), Ok(0x1_0000), "{offset:#x}");
        }
        // Software disabled after reset: masked writes work, unmask is refused.
        for offset in LVT_REGS {
            write(&mut bus, offset, 0x1_00fe).unwrap();
            assert_eq!(read(&mut bus, offset), Ok(0x1_00fe));
            let before = snapshot(&apic);
            assert_eq!(write(&mut bus, offset, 0xfe), Err(error(UC)), "{offset:#x}");
            assert_eq!(snapshot(&apic), before);
        }
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        // Linux values: LINT0 ExtINT, LINT1 NMI, Error vector 0xfe; each also
        // masked and then restored unmasked.
        for (offset, value) in [(0x350, 0x700), (0x360, 0x400), (0x370, 0xfe)] {
            write(&mut bus, offset, value).unwrap();
            assert_eq!(read(&mut bus, offset), Ok(value));
            write(&mut bus, offset, value | 0x1_0000).unwrap();
            assert_eq!(read(&mut bus, offset), Ok(value | 0x1_0000));
            write(&mut bus, offset, value).unwrap();
            assert_eq!(read(&mut bus, offset), Ok(value));
        }
        // LINT entries keep polarity, trigger and every valid delivery mode;
        // Delivery Status (12) and Remote IRR (14) are dropped on write.
        for offset in [0x350, 0x360] {
            for mode in [0u64, 2, 4, 5, 7] {
                let value = 0xa000 | (mode << 8) | 0x33;
                write(&mut bus, offset, value | 0x5000).unwrap();
                assert_eq!(read(&mut bus, offset), Ok(value), "{offset:#x} mode {mode}");
            }
        }
        write(&mut bus, 0x370, 0x10fe).unwrap();
        assert_eq!(read(&mut bus, 0x370), Ok(0xfe));
        // Vector 0 and < 16 are stored: nothing is delivered from these LVTs.
        write(&mut bus, 0x360, 0x1_0005).unwrap();
        assert_eq!(read(&mut bus, 0x360), Ok(0x1_0005));
    }

    #[test]
    fn storage_lvts_have_no_effect_on_requests() {
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x41).unwrap();
        let irr = |a: &LocalApicHandle| {
            let s = snapshot(a).cpus[0].local.clone();
            (s.irr, s.isr, s.tmr)
        };
        let before = irr(&apic);
        for offset in LVT_REGS {
            for value in [0xfe, 0x1_00fe] {
                write(&mut bus, offset, value).unwrap();
                assert_eq!(irr(&apic), before);
            }
        }
        assert_eq!(apic.pending(), Some(0x41));
    }

    #[test]
    fn software_disable_masks_new_lvts_and_unmask_is_refused_until_enable() {
        let (mut bus, apic) = setup();
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        for offset in LVT_REGS {
            write(&mut bus, offset, 0x31).unwrap();
        }
        write(&mut bus, 0xf0, 0xff).unwrap();
        for offset in LVT_REGS {
            assert_eq!(read(&mut bus, offset), Ok(0x1_0031), "{offset:#x}");
        }
        let before = snapshot(&apic);
        for offset in LVT_REGS {
            assert_eq!(write(&mut bus, offset, 0x31), Err(error(UC)));
        }
        assert_eq!(snapshot(&apic), before);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        write(&mut bus, 0x350, 0x31).unwrap();
        assert_eq!(read(&mut bus, 0x350), Ok(0x31));
    }

    #[test]
    fn esr_write_latches_accumulated_errors_and_zero_state_is_stable() {
        let (mut bus, apic) = setup();
        // No source: the log is empty, so read-after-write is always 0.
        assert_eq!(read(&mut bus, 0x280), Ok(0));
        write(&mut bus, 0x280, 0).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0));
        // The mechanism with an injected error log.
        apic.0.lock().unwrap().cpus[0].local.esr_pending = 0x20;
        assert_eq!(read(&mut bus, 0x280), Ok(0), "read alone does not latch");
        write(&mut bus, 0x280, 0).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0x20));
        assert_eq!(read(&mut bus, 0x280), Ok(0x20), "read does not clear");
        assert_eq!(snapshot(&apic).cpus[0].local.esr_pending, 0);
        // The value written is ignored; the next write clears the latch.
        write(&mut bus, 0x280, 0xdead_beef).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0));
        // A nonzero value does not inject errors or retain the latch.
        apic.0.lock().unwrap().cpus[0].local.esr_pending = 0x08;
        write(&mut bus, 0x280, 0xffff_ffff).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0x08));
        write(&mut bus, 0x280, 0).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0));
    }

    #[test]
    fn esr_write_does_not_touch_irr_isr_tmr_or_other_registers() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        dirty(&mut bus, &apic);
        apic.0.lock().unwrap().cpus[0].local.esr_pending = 0;
        let mut before = snapshot(&apic);
        write(&mut bus, 0x280, 0).unwrap();
        before.cpus[0].local.esr = 0;
        assert_eq!(snapshot(&apic), before);
        // Only the latch moves when a log exists.
        apic.0.lock().unwrap().cpus[0].local.esr_pending = 4;
        write(&mut bus, 0x280, 0).unwrap();
        let after = snapshot(&apic);
        assert_eq!(after.cpus[0].local.irr, before.cpus[0].local.irr);
        assert_eq!(after.cpus[0].local.isr, before.cpus[0].local.isr);
        assert_eq!(after.cpus[0].local.tmr, before.cpus[0].local.tmr);
        assert_eq!(
            (after.cpus[0].local.esr, after.cpus[0].local.esr_pending),
            (4, 0)
        );
    }

    #[test]
    fn dfr_and_ldr_store_without_routing() {
        let (mut bus, apic) = setup();
        assert_eq!(read(&mut bus, 0xe0), Ok(0xffff_ffff));
        assert_eq!(read(&mut bus, 0xd0), Ok(0));
        write(&mut bus, 0xe0, 0x0fff_ffff).unwrap(); // cluster model
        assert_eq!(read(&mut bus, 0xe0), Ok(0x0fff_ffff));
        write(&mut bus, 0xe0, 0xffff_ffff).unwrap(); // flat model
        assert_eq!(read(&mut bus, 0xe0), Ok(0xffff_ffff));
        write(&mut bus, 0xe0, 0x0000_0000).unwrap(); // reserved bits read as 1
        assert_eq!(read(&mut bus, 0xe0), Ok(0x0fff_ffff));
        write(&mut bus, 0xd0, 0x0100_0000).unwrap();
        assert_eq!(read(&mut bus, 0xd0), Ok(0x0100_0000));
        // Logical ID does not affect requests or pending vectors.
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x41).unwrap();
        assert_eq!(apic.eligible(), Some(0x41));
        let before = snapshot(&apic);
        for (offset, value) in [(0xe0, 0x1fff_ffff), (0xd0, 1)] {
            assert_eq!(write(&mut bus, offset, value), Err(error(UC)));
        }
        assert_eq!(snapshot(&apic), before);
    }

    #[test]
    fn thermal_and_cmci_stay_unknown_offset() {
        // Max LVT = 4 does not include them; no CPUID profile may advertise thermal monitoring
        // or CMCI while they are absent (`absent_thermal_and_cmci_lvts_match_the_cpu_identity`).
        let (mut bus, timer) = timer_setup(1_000_000_000);
        let (mut plain_bus, plain) = setup();
        for (bus, apic) in [(&mut bus, &timer), (&mut plain_bus, &plain)] {
            let before = snapshot(apic);
            for offset in [0x2f0, 0x330] {
                assert_eq!(
                    read(bus, offset),
                    Err(error(DeviceError::UnknownOffset)),
                    "{offset:#x}"
                );
                for value in [0, 0x1_0000, u64::MAX] {
                    assert_eq!(
                        write(bus, offset, value),
                        Err(error(DeviceError::UnknownOffset))
                    );
                }
            }
            assert_eq!(snapshot(apic), before);
        }
    }

    // ---- S7-32 slice M: LVT Performance Monitoring Counters -------------------

    /// SDM Vol. 3A 13.4.7.1 (an LVT resets to 0 except the mask), Figure 13-8 (PMC entry: vector,
    /// delivery mode, read-only Delivery Status, mask) and 13.5.1 (Fixed, SMI and NMI; the mask
    /// is cleared only by software).
    #[test]
    fn pmc_lvt_resets_masked_and_stores_vector_delivery_mode_and_mask() {
        let (mut bus, apic) = setup();
        assert_eq!(read(&mut bus, 0x340), Ok(0x1_0000));
        // Software disabled after reset: a masked write is stored, an unmasked one refused.
        write(&mut bus, 0x340, 0x1_00d8).unwrap();
        assert_eq!(read(&mut bus, 0x340), Ok(0x1_00d8));
        let before = snapshot(&apic);
        assert_eq!(write(&mut bus, 0x340, 0xd8), Err(error(UC)));
        assert_eq!(snapshot(&apic), before);
        write(&mut bus, 0xf0, 0x1ff).unwrap();
        // Fixed, SMI and NMI, each unmasked and masked.
        for mode in [0b000u64, 0b010, 0b100] {
            let value = (mode << 8) | 0xfe;
            for value in [value, value | 0x1_0000, value] {
                write(&mut bus, 0x340, value).unwrap();
                assert_eq!(read(&mut bus, 0x340), Ok(value), "{value:#x}");
            }
        }
        // Delivery Status (12) is read-only: dropped on write, it reads 0 (Idle).
        write(&mut bus, 0x340, 0x10fe).unwrap();
        assert_eq!(read(&mut bus, 0x340), Ok(0xfe));
        // Vector 0 and < 16 are stored: nothing is delivered from this LVT.
        write(&mut bus, 0x340, 0x5).unwrap();
        assert_eq!(read(&mut bus, 0x340), Ok(0x5));
        // Linux `perf_events_lapic_init` (written only with a PMU driver): NMI.
        write(&mut bus, 0x340, 0x400).unwrap();
        assert_eq!(read(&mut bus, 0x340), Ok(0x400));
    }

    /// ISSUE-700: the writes of the Windows HAL local-unit initialisation (`ntkrnlmp` +
    /// `0x569990`, in its order; the Error LVT value is the one of the MINIEX copy of the
    /// sequence, `+0xbe92ad`) and of its PMI line (`+0x33e19d`: Fixed, vector 0xFE, first written
    /// as the disabled line `0x100FF`).
    #[test]
    fn windows_hal_local_unit_initialisation_is_accepted() {
        let (mut bus, apic) = timer_setup(1_000_000_000);
        for (offset, value) in [
            (0xf0, 0x1df),
            (0x320, 0x3_00d8),
            (0x3e0, 0xb),
            (0x370, 0xe2),
            (0x340, 0x1_00d8),
            (0x350, 0x1_0000),
            (0x350, 0x1_00d8),
            (0x360, 0x1_84d8),
        ] {
            write(&mut bus, offset, value).unwrap();
            assert_eq!(read(&mut bus, offset), Ok(value), "{offset:#x}");
        }
        assert_eq!(read(&mut bus, 0x20), Ok(0));
        for value in [0x1_00ff, 0xfe] {
            write(&mut bus, 0x340, value).unwrap();
            assert_eq!(read(&mut bus, 0x340), Ok(value));
        }
        assert_eq!(apic.pending(), None);
    }

    /// Linux 7.0 with Max LVT = 4 (`arch/x86/kernel/apic/apic.c`): `lapic_setup_esr` clears ESR
    /// before and after it enables the Error LVT (`maxlvt > 3`); `clear_local_APIC` masks the PMC
    /// entry too (`maxlvt >= 4`) and touches neither Thermal (`>= 5`) nor CMCI (`>= 6`).
    #[test]
    fn linux_maxlvt_four_paths_are_accepted() {
        let (mut bus, _apic) = timer_setup(1_000_000_000);
        // lapic_setup_esr
        write(&mut bus, 0x280, 0).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0));
        write(&mut bus, 0x370, 0xfe).unwrap();
        write(&mut bus, 0x280, 0).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0));
        // clear_local_APIC
        write(&mut bus, 0x370, 0x1_00fe).unwrap();
        for offset in [0x320, 0x350, 0x360, 0x340] {
            let v = read(&mut bus, offset).unwrap();
            write(&mut bus, offset, v | 0x1_0000).unwrap();
        }
        for offset in [0x320, 0x350, 0x360, 0x370, 0x340] {
            write(&mut bus, offset, 0x1_0000).unwrap();
        }
        write(&mut bus, 0x280, 0).unwrap();
        assert_eq!(read(&mut bus, 0x280), Ok(0));
        for offset in [0x320, 0x340, 0x350, 0x360, 0x370] {
            assert_eq!(read(&mut bus, offset), Ok(0x1_0000), "{offset:#x}");
        }
    }

    /// Linux 7.0 starts an AP with Max LVT > 3 (`arch/x86/kernel/smpboot.c`): `send_init_sequence`
    /// (line 832) clears ESR and reads it, then sends INIT assert (`0xC500`) and deassert
    /// (`0x8500`); `wakeup_secondary_cpu_via_init` (lines 880, 916) twice clears ESR, reads it,
    /// sends STARTUP (`0x600 | vector`), clears ESR again and requires `ESR & 0xEF == 0`. That is
    /// five ESR writes on the boot processor per AP beyond the unconditional one in `do_boot_cpu`
    /// (line 1037), which Max LVT = 3 also makes; the AP ends with the INIT and the SIPI pending,
    /// and the boot processor's Delivery Status stays Idle.
    #[test]
    fn linux_ap_startup_with_maxlvt_four_clears_esr_around_init_and_startup() {
        fn clear_esr(device: &mut LocalApic) -> u64 {
            device.write_from_cpu(0, dword(0x280), 0).unwrap();
            device.read_from_cpu(0, dword(0x280)).unwrap()
        }
        fn icr_write(device: &mut LocalApic, low: u64) {
            device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
            device.write_from_cpu(0, dword(0x300), low).unwrap();
            assert_eq!(
                device.read_from_cpu(0, dword(0x300)).unwrap() & (1 << 12),
                0
            );
        }
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0xf0), 0x1ff).unwrap();
        assert_eq!((VERSION >> 16) & 0xff, 4, "the path of Max LVT > 3");
        let mut esr_writes = 0;
        assert_eq!(clear_esr(&mut device), 0);
        esr_writes += 1;
        icr_write(&mut device, icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0));
        icr_write(&mut device, icr(INIT, PHYSICAL, NO_SHORTHAND, LT_01, 0));
        for _ in 0..2 {
            assert_eq!(clear_esr(&mut device), 0);
            icr_write(&mut device, icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, 0x9a));
            assert_eq!(clear_esr(&mut device) & 0xef, 0);
            esr_writes += 2;
        }
        assert_eq!(esr_writes, 5);
        assert_eq!(
            startup_of(&device, 1),
            PendingStartup {
                init: true,
                sipi: Some(0x9a)
            }
        );
        assert_eq!(startup_of(&device, 0), PendingStartup::default());
    }

    /// Every CPUID profile, in declaration order. The successor of each variant is an exhaustive
    /// match without a wildcard, so a new variant does not compile until it is placed in the
    /// chain, and then the identity check below covers it.
    fn every_cpuid_profile() -> Vec<od_isa::cpuid::CpuidProfile> {
        use od_isa::cpuid::CpuidProfile as P;
        let next = |profile: &P| match profile {
            P::DiagnosticV1 => Some(P::X8664V1Stage4A),
            P::X8664V1Stage4A => Some(P::X8664V1Stage4AApic),
            P::X8664V1Stage4AApic => Some(P::X8664V1Stage4AExt1),
            P::X8664V1Stage4AExt1 => Some(P::X8664V1Stage4AExt2),
            P::X8664V1Stage4AExt2 => Some(P::OdX8664V1Neutral),
            P::OdX8664V1Neutral => Some(P::OdX8664V2Windows),
            P::OdX8664V2Windows => None,
        };
        std::iter::successors(Some(P::DiagnosticV1), next).collect()
    }

    /// Thermal Sensor `0x330` and CMCI `0x2f0` are absent (S7-32 slice M). That is sound only
    /// while no CPUID profile reports what makes a guest program them: ACPI (leaf 1 EDX 22) and
    /// TM (EDX 29) for Linux `intel_thermal_supported`, TM2 (ECX 8), HWP with its notification or
    /// HFI (leaf 6 EAX 7, 8, 19) for the Windows `intelppm` thermal interrupt, and MCG_CMCI_P
    /// (IA32_MCG_CAP bit 10) for the Windows and Linux CMCI set-up.
    #[test]
    fn absent_thermal_and_cmci_lvts_match_the_cpu_identity() {
        let hint =
            "declare the Thermal/CMCI LVT with Max LVT first (MACHINE_CONTRACT, S7-32 slice M)";
        for profile in every_cpuid_profile() {
            let one = profile.query(1, 0);
            assert_eq!(one.edx & (1 << 22 | 1 << 29), 0, "{profile:?}: {hint}");
            assert_eq!(one.ecx & (1 << 8), 0, "{profile:?}: {hint}");
            let six = profile.query(6, 0);
            assert_eq!(
                six.eax & (1 << 7 | 1 << 8 | 1 << 19),
                0,
                "{profile:?}: {hint}"
            );
        }
        assert_eq!(od_isa::msr::IA32_MCG_CAP_VALUE & (1 << 10), 0, "{hint}");
    }

    #[test]
    fn new_registers_reject_bad_width_space_and_alignment_without_change() {
        use AccessWidth::{Byte, Dword, Qword, Word};
        let (mut bus, apic) = timer_setup(1_000_000_000);
        dirty(&mut bus, &apic);
        let before = snapshot(&apic);
        let mut device = LocalApic(Arc::clone(&apic.0));
        for offset in [0x30u64, 0x280, 0x340, 0x350, 0x360, 0x370, 0xd0, 0xe0] {
            for (space, width, delta, expected) in [
                (IoSpace::Pio, Dword, 0, DeviceError::UnsupportedSpace),
                (IoSpace::Mmio, Byte, 0, DeviceError::UnsupportedWidth),
                (IoSpace::Mmio, Word, 0, DeviceError::UnsupportedWidth),
                (IoSpace::Mmio, Qword, 0, DeviceError::UnsupportedWidth),
                (IoSpace::Mmio, Dword, 4, DeviceError::Misaligned),
                (IoSpace::Mmio, Dword, 1, DeviceError::Misaligned),
            ] {
                let access = DeviceAccess {
                    space,
                    offset: offset + delta,
                    width,
                };
                assert_eq!(device.read(access), Err(expected.clone()));
                assert_eq!(device.write(access, 0), Err(expected));
            }
        }
        assert_eq!(snapshot(&apic), before);
    }

    // S6-11: one Local APIC per vCPU behind the one window.

    fn dword(offset: u64) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width: AccessWidth::Dword,
        }
    }

    fn smp_timer_device(count: usize) -> (LocalApic, crate::VirtualClockHandle) {
        let clock = crate::VirtualClockHandle::new(1_000_000_000).unwrap();
        let device = LocalApic::with_shared_timers(clock.clone(), 1_000_000_000, count).unwrap();
        (device, clock)
    }

    #[test]
    fn apic_id_register_is_the_index_of_the_vcpu_that_reads_it() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        for cpu in 0..3 {
            assert_eq!(
                device.read_from_cpu(cpu, dword(0x20)),
                Ok((cpu as u64) << 24),
                "vcpu {cpu}"
            );
        }
        // The plain bus access is the boot processor's.
        assert_eq!(device.read(dword(0x20)), Ok(0));
        // The ID is read-only for every processor.
        assert_eq!(
            device.write_from_cpu(1, dword(0x20), 0x0200_0000),
            Err(DeviceError::ReadOnly)
        );
        assert_eq!(device.read_from_cpu(1, dword(0x20)), Ok(1 << 24));
    }

    #[test]
    fn an_access_from_a_vcpu_the_board_does_not_have_is_refused() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        let before = device.handle().0.lock().unwrap().clone();
        assert_eq!(
            device.read_from_cpu(2, dword(0x20)),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            device.write_from_cpu(2, dword(0x80), 0x30),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(*device.handle().0.lock().unwrap(), before);
        assert!(device.handle_for(2).is_none());
        assert_eq!(device.handle().cpu_count(), 2);
        // The board refuses a processor count outside 1..=4.
        for count in [0, 5] {
            assert_eq!(
                LocalApic::with_cpus(count).err(),
                Some(DeviceError::UnsupportedConfiguration)
            );
        }
    }

    #[test]
    fn registers_written_by_one_vcpu_are_invisible_to_the_other() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(1, dword(0x80), 0x30).unwrap();
        device.write_from_cpu(1, dword(0xf0), 0x1ab).unwrap();
        device.write_from_cpu(1, dword(0xd0), 0x0300_0000).unwrap();
        assert_eq!(device.read_from_cpu(1, dword(0x80)), Ok(0x30));
        assert_eq!(device.read_from_cpu(1, dword(0xf0)), Ok(0x1ab));
        assert_eq!(device.read_from_cpu(1, dword(0xd0)), Ok(0x0300_0000));
        assert_eq!(device.read_from_cpu(0, dword(0x80)), Ok(0));
        assert_eq!(device.read_from_cpu(0, dword(0xf0)), Ok(0xff));
        assert_eq!(device.read_from_cpu(0, dword(0xd0)), Ok(0));
        // The handles follow the same split.
        assert_eq!(device.handle_for(1).unwrap().with_state(|s| s.tpr), 0x30);
        assert_eq!(device.handle().with_state(|s| s.tpr), 0);
    }

    #[test]
    fn a_request_and_an_eoi_stay_inside_their_own_lapic() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        let (cpu0, cpu1) = (device.handle(), device.handle_for(1).unwrap());
        for cpu in 0..2 {
            device.write_from_cpu(cpu, dword(0xf0), 0x1ff).unwrap();
        }
        cpu1.request_fixed_edge(0x61).unwrap();
        assert_eq!((cpu0.pending(), cpu1.pending()), (None, Some(0x61)));
        // cpu 1 takes the request; cpu 0 has nothing eligible and acknowledges nothing.
        assert_eq!(cpu0.acknowledge(), None);
        assert_eq!(cpu1.acknowledge(), Some(0x61));
        assert_eq!(cpu1.vector_state(0x61), (false, true));
        assert_eq!(cpu0.vector_state(0x61), (false, false));
        // The same vector requested on cpu 0 is its own request, not a repeat of cpu 1's.
        cpu0.request_fixed_edge(0x61).unwrap();
        assert_eq!(cpu0.vector_state(0x61), (true, false));
        assert_eq!(cpu1.vector_state(0x61), (false, true));
        // Disabling one LAPIC clears only its own state.
        cpu1.set_global_enabled(false).unwrap();
        assert_eq!(cpu1.vector_state(0x61), (false, false));
        assert_eq!(cpu0.vector_state(0x61), (true, false));
        assert_eq!(
            cpu1.request_fixed_edge(0x62),
            Err(DeviceError::UnsupportedConfiguration)
        );
        cpu0.request_fixed_edge(0x62).unwrap();
    }

    #[test]
    fn the_timer_of_one_vcpu_does_not_change_the_state_of_the_other() {
        let (mut device, clock) = smp_timer_device(2);
        let (cpu0, cpu1) = (device.handle(), device.handle_for(1).unwrap());
        for cpu in 0..2 {
            device.write_from_cpu(cpu, dword(0xf0), 0x1ff).unwrap();
        }
        // Only vcpu 1 starts its timer: unmasked vector 0x40, one-shot, count 1 (2 ns).
        device.write_from_cpu(1, dword(0x320), 0x40).unwrap();
        device.write_from_cpu(1, dword(0x380), 1).unwrap();
        let cpu0_before = cpu0.0.lock().unwrap().cpus[0].clone();
        assert!(cpu1.next_timer_deadline().is_some());
        assert_eq!(cpu0.next_timer_deadline(), None);
        clock.advance_standalone(10).unwrap();
        // The expiry belongs to vcpu 1 alone.
        assert_eq!(cpu0.dispatch_timer(), Ok(false));
        assert_eq!(cpu0.0.lock().unwrap().cpus[0], cpu0_before);
        assert_eq!(cpu1.dispatch_timer(), Ok(true));
        assert_eq!(cpu1.vector_state(0x40), (true, false));
        assert_eq!(cpu0.vector_state(0x40), (false, false));
        assert_eq!(cpu0.0.lock().unwrap().cpus[0], cpu0_before);
        // The timer registers are per processor too: vcpu 0 still reads its masked reset LVT,
        // no count, and a current count of zero.
        assert_eq!(device.read_from_cpu(0, dword(0x320)), Ok(u64::from(MASK)));
        assert_eq!(device.read_from_cpu(0, dword(0x380)), Ok(0));
        assert_eq!(device.read_from_cpu(1, dword(0x380)), Ok(1));
        // A second expiry now on vcpu 0 does not disturb the already pending request of vcpu 1.
        device.write_from_cpu(0, dword(0x320), 0x50).unwrap();
        device.write_from_cpu(0, dword(0x380), 1).unwrap();
        clock.advance_standalone(10).unwrap();
        let cpu1_before = cpu1.0.lock().unwrap().cpus[1].clone();
        assert_eq!(cpu0.dispatch_timer(), Ok(true));
        assert_eq!(cpu0.vector_state(0x50), (true, false));
        assert_eq!(cpu1.0.lock().unwrap().cpus[1], cpu1_before);
    }

    #[test]
    fn resetting_one_lapic_leaves_the_other_alone() {
        let (mut device, _clock) = smp_timer_device(2);
        let (cpu0, cpu1) = (device.handle(), device.handle_for(1).unwrap());
        for cpu in 0..2 {
            device.write_from_cpu(cpu, dword(0xf0), 0x1ff).unwrap();
            device.write_from_cpu(cpu, dword(0x320), 0x40).unwrap();
        }
        cpu0.request_fixed_edge(0x70).unwrap();
        cpu1.request_fixed_edge(0x70).unwrap();
        let cpu0_before = cpu0.0.lock().unwrap().cpus[0].clone();
        cpu1.reset().unwrap();
        assert_eq!(cpu1.pending(), None);
        assert_eq!(cpu1.timer_vector(), None);
        assert_eq!(cpu0.0.lock().unwrap().cpus[0], cpu0_before);
        assert_eq!(cpu0.pending(), Some(0x70));
    }

    // S6-12: interprocessor interrupts by the matrix of SMP_CONTRACT.md 5.

    const FIXED: u64 = 0;
    const PHYSICAL: u64 = 0;
    const LOGICAL: u64 = 1 << 11;
    const NO_SHORTHAND: u64 = 0;
    const SELF_SH: u64 = 1 << 18;
    const ALL_SH: u64 = 2 << 18;
    const OTHERS_SH: u64 = 3 << 18;
    /// `(Level, Trigger)` as `(0,0)`, `(1,0)`, `(0,1)`, `(1,1)`.
    const LT_00: u64 = 0;
    const LT_10: u64 = 1 << 14;
    const LT_01: u64 = 1 << 15;
    const LT_11: u64 = 3 << 14;

    fn icr(mode: u64, dest_mode: u64, shorthand: u64, level_trigger: u64, vector: u8) -> u64 {
        (mode << 8) | dest_mode | shorthand | level_trigger | u64::from(vector)
    }

    /// Which vCPUs of `device` hold `vector` in their IRR.
    fn holders(device: &LocalApic, vector: u8) -> Vec<usize> {
        (0..device.handle().cpu_count())
            .filter(|&cpu| device.handle_for(cpu).unwrap().vector_state(vector).0)
            .collect()
    }

    /// `command` written by vCPU `sender` is refused and no state of any LAPIC changes.
    fn assert_ipi_refused(device: &mut LocalApic, sender: usize, command: u64) {
        let before = device.handle().0.lock().unwrap().clone();
        assert_eq!(
            device.write_from_cpu(sender, dword(0x300), command),
            Err(DeviceError::UnsupportedConfiguration),
            "{command:#x}"
        );
        assert_eq!(*device.handle().0.lock().unwrap(), before, "{command:#x}");
    }

    #[test]
    fn a_targeted_ipi_of_vcpu_0_is_pending_on_vcpu_1_and_not_on_vcpu_0() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let command = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(holders(&device, 0x61), [1]);
        assert_eq!(device.handle_for(1).unwrap().pending(), Some(0x61));
        assert_eq!(device.handle().pending(), None);
        // The sender keeps the command it wrote; the target's ICR is untouched.
        assert_eq!(device.read_from_cpu(0, dword(0x300)), Ok(command));
        assert_eq!(device.read_from_cpu(1, dword(0x300)), Ok(0));
        // The target's own acknowledge cycle takes it; the sender has nothing to take.
        device.write_from_cpu(1, dword(0xf0), 0x1ff).unwrap();
        device.write_from_cpu(0, dword(0xf0), 0x1ff).unwrap();
        assert_eq!(device.handle().acknowledge(), None);
        assert_eq!(device.handle_for(1).unwrap().acknowledge(), Some(0x61));
    }

    #[test]
    fn physical_destination_matrix_cell_delivers_every_legal_vector_to_the_named_vcpu() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        device.write_from_cpu(2, dword(0x310), 1 << 24).unwrap();
        for vector in [0x10u8, 0x11, 0x7f, 0xef, 0xfe, 0xff] {
            let command = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, vector);
            device.write_from_cpu(2, dword(0x300), command).unwrap();
            assert_eq!(holders(&device, vector), [1], "{vector:#x}");
        }
        // The destination is read from the sender's ICR_HIGH, not the target's.
        device.write_from_cpu(2, dword(0x310), 0).unwrap();
        let command = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x30);
        device.write_from_cpu(2, dword(0x300), command).unwrap();
        assert_eq!(holders(&device, 0x30), [0]);
        // A destination equal to the sender's own APIC ID delivers to itself.
        device.write_from_cpu(2, dword(0x310), 2 << 24).unwrap();
        device.write_from_cpu(2, dword(0x300), command).unwrap();
        assert_eq!(holders(&device, 0x30), [0, 2]);
    }

    #[test]
    fn self_shorthand_matrix_cell_delivers_to_the_sender_for_both_encodings() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        for (sender, level_trigger, vector) in [
            (0, LT_00, 0x40),
            (0, LT_10, 0x41),
            (1, LT_00, 0x42),
            (1, LT_10, 0x43),
        ] {
            // Destination byte and Destination Mode are ignored by a shorthand.
            device
                .write_from_cpu(sender, dword(0x310), 0xee << 24)
                .unwrap();
            let command = icr(FIXED, LOGICAL, SELF_SH, level_trigger, vector);
            device
                .write_from_cpu(sender, dword(0x300), command)
                .unwrap();
            assert_eq!(holders(&device, vector), [sender], "{vector:#x}");
        }
    }

    #[test]
    fn all_including_self_matrix_cell_delivers_to_every_vcpu() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        device.write_from_cpu(1, dword(0x310), 0xee << 24).unwrap();
        let command = icr(FIXED, LOGICAL, ALL_SH, LT_00, 0x50);
        device.write_from_cpu(1, dword(0x300), command).unwrap();
        assert_eq!(holders(&device, 0x50), [0, 1, 2]);
        // With one vCPU it is the sender alone.
        let mut single = LocalApic::with_cpus(1).unwrap();
        single.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(holders(&single, 0x50), [0]);
    }

    #[test]
    fn all_but_self_matrix_cell_delivers_to_every_other_vcpu() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        for sender in 0..3 {
            let vector = 0x60 + sender as u8;
            let command = icr(FIXED, PHYSICAL, OTHERS_SH, LT_00, vector);
            device
                .write_from_cpu(sender, dword(0x300), command)
                .unwrap();
            let others: Vec<usize> = (0..3).filter(|&cpu| cpu != sender).collect();
            assert_eq!(holders(&device, vector), others, "sender {sender}");
        }
        // With one vCPU there is nobody to deliver to: accepted, no request anywhere.
        let mut single = LocalApic::with_cpus(1).unwrap();
        let command = icr(FIXED, PHYSICAL, OTHERS_SH, LT_00, 0x60);
        single.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(holders(&single, 0x60), Vec::<usize>::new());
        assert_eq!(single.read_from_cpu(0, dword(0x300)), Ok(command));
    }

    #[test]
    fn delivery_status_is_ignored_on_write_and_reads_idle() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let command = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x70);
        device
            .write_from_cpu(0, dword(0x300), command | 0x1000)
            .unwrap();
        assert_eq!(holders(&device, 0x70), [1]);
        assert_eq!(device.read_from_cpu(0, dword(0x300)), Ok(command));
        // Idle also right after delivery to every vCPU.
        let all = icr(FIXED, PHYSICAL, ALL_SH, LT_00, 0x71);
        device.write_from_cpu(0, dword(0x300), all).unwrap();
        assert_eq!(device.read_from_cpu(0, dword(0x300)).unwrap() & 0x1000, 0);
        // A refused command does not set it either.
        let refused = icr(FIXED, PHYSICAL, ALL_SH, LT_11, 0x72) | 0x1000;
        assert_ipi_refused(&mut device, 0, refused);
        assert_eq!(device.read_from_cpu(0, dword(0x300)), Ok(all));
    }

    #[test]
    fn matrix_cells_with_a_wrong_level_trigger_pair_are_refused() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        for (shorthand, allowed) in [
            (NO_SHORTHAND, vec![LT_00]),
            (SELF_SH, vec![LT_00, LT_10]),
            (ALL_SH, vec![LT_00]),
            (OTHERS_SH, vec![LT_00]),
        ] {
            for level_trigger in [LT_00, LT_10, LT_01, LT_11] {
                let command = icr(FIXED, PHYSICAL, shorthand, level_trigger, 0x80);
                if allowed.contains(&level_trigger) {
                    device.write_from_cpu(0, dword(0x300), command).unwrap();
                } else {
                    assert_ipi_refused(&mut device, 0, command);
                }
            }
        }
    }

    #[test]
    fn matrix_cells_marked_as_an_error_are_refused_before_any_state_changes() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        // Logical destination without shorthand.
        assert_ipi_refused(
            &mut device,
            0,
            icr(FIXED, LOGICAL, NO_SHORTHAND, LT_00, 0x61),
        );
        // No vCPU has this APIC ID, including the xAPIC broadcast ID.
        for destination in [2u64, 3, 0x7f, 0xfe, 0xff] {
            device
                .write_from_cpu(0, dword(0x310), destination << 24)
                .unwrap();
            assert_ipi_refused(
                &mut device,
                0,
                icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61),
            );
        }
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        // Vectors below 0x10 for every addressing form.
        for shorthand in [NO_SHORTHAND, SELF_SH, ALL_SH, OTHERS_SH] {
            for vector in [0u8, 1, 2, 0x0f] {
                assert_ipi_refused(
                    &mut device,
                    0,
                    icr(FIXED, PHYSICAL, shorthand, LT_00, vector),
                );
            }
        }
        // Lowest Priority, SMI and the reserved modes 011 and 111 with every addressing form
        // and every pair; NMI, INIT and SIPI outside the rows the matrix lists. S7-32: Lowest
        // Priority without shorthand and with the edge pair is accepted (one target).
        for mode in [1u64, 2, 3, 7] {
            for shorthand in [NO_SHORTHAND, SELF_SH, ALL_SH, OTHERS_SH] {
                for level_trigger in [LT_00, LT_10, LT_01, LT_11] {
                    if mode == 1
                        && shorthand == NO_SHORTHAND
                        && matches!(level_trigger, LT_00 | LT_10)
                    {
                        continue;
                    }
                    assert_ipi_refused(
                        &mut device,
                        0,
                        icr(mode, PHYSICAL, shorthand, level_trigger, 0x61),
                    );
                }
            }
        }
        for mode in [4u64, 5, 6] {
            for shorthand in [SELF_SH, ALL_SH] {
                assert_ipi_refused(&mut device, 0, icr(mode, PHYSICAL, shorthand, LT_00, 0));
            }
        }
        for mode in [5u64, 6] {
            assert_ipi_refused(&mut device, 0, icr(mode, PHYSICAL, OTHERS_SH, LT_00, 0));
        }
        // Reserved ICR bits, also on an otherwise legal command; Delivery Status does not
        // legalize them.
        let legal = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61);
        for bit in [13u32, 16, 17, 20, 24, 31, 32] {
            assert_ipi_refused(&mut device, 0, legal | (1 << bit));
            assert_ipi_refused(&mut device, 0, legal | 0x1000 | (1 << bit));
        }
        // The same legal command is accepted afterwards.
        device.write_from_cpu(0, dword(0x300), legal).unwrap();
        assert_eq!(holders(&device, 0x61), [1]);
    }

    // S6-12b: NMI waits as one latch in the LAPIC of the target.

    const NMI: u64 = 4;

    fn nmi_holders(device: &LocalApic) -> Vec<usize> {
        (0..device.handle().cpu_count())
            .filter(|&cpu| device.handle_for(cpu).unwrap().pending_nmi())
            .collect()
    }

    #[test]
    fn nmi_physical_is_pending_in_the_target_lapic_only_and_ignores_the_vector_field() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        device.write_from_cpu(0, dword(0x310), 2 << 24).unwrap();
        // Linux emits vector 0; any value is ignored (SMP_CONTRACT.md 5).
        for vector in [0u8, 0x44] {
            let command = icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, vector);
            device.write_from_cpu(0, dword(0x300), command).unwrap();
            assert_eq!(nmi_holders(&device), [2], "{vector:#x}");
            // Delivery Status reads Idle, the sender keeps the command, no IRR bit is raised and no
            // INIT or SIPI is pending: the NMI is a separate event.
            assert_eq!(device.read_from_cpu(0, dword(0x300)), Ok(command));
            assert_eq!(holders(&device, vector), Vec::<usize>::new());
            assert_eq!(startup_of(&device, 2), PendingStartup::default());
            assert!(device.handle_for(2).unwrap().take_nmi());
            assert_eq!(nmi_holders(&device), Vec::<usize>::new());
        }
        // A target may be the sender itself: physical addressing names an APIC ID.
        device.write_from_cpu(0, dword(0x310), 0).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, 0))
            .unwrap();
        assert_eq!(nmi_holders(&device), [0]);
    }

    #[test]
    fn nmi_all_but_self_is_pending_in_every_other_lapic_and_not_in_the_sender() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        // The destination byte and Destination Mode are ignored with a shorthand.
        device.write_from_cpu(1, dword(0x310), 0xff << 24).unwrap();
        for command in [
            icr(NMI, PHYSICAL, OTHERS_SH, LT_00, 0),
            icr(NMI, 1 << 11, OTHERS_SH, LT_00, 0),
        ] {
            device.write_from_cpu(1, dword(0x300), command).unwrap();
            assert_eq!(nmi_holders(&device), [0, 2], "{command:#x}");
            for cpu in [0, 2] {
                assert!(device.handle_for(cpu).unwrap().take_nmi());
            }
        }
        // One vCPU has no other target: accepted, nothing pending.
        let mut single = LocalApic::with_cpus(1).unwrap();
        single
            .write_from_cpu(0, dword(0x300), icr(NMI, PHYSICAL, OTHERS_SH, LT_00, 0))
            .unwrap();
        assert_eq!(nmi_holders(&single), Vec::<usize>::new());
    }

    #[test]
    fn nmi_self_all_and_every_other_encoding_are_refused_without_a_change() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        // NMI Self and All, with every pair.
        for shorthand in [SELF_SH, ALL_SH] {
            for level_trigger in [LT_00, LT_10, LT_01, LT_11] {
                assert_ipi_refused(
                    &mut device,
                    0,
                    icr(NMI, PHYSICAL, shorthand, level_trigger, 0),
                );
            }
        }
        // Pairs the row does not name, for both rows.
        for shorthand in [NO_SHORTHAND, OTHERS_SH] {
            for level_trigger in [LT_10, LT_01, LT_11] {
                assert_ipi_refused(
                    &mut device,
                    0,
                    icr(NMI, PHYSICAL, shorthand, level_trigger, 0),
                );
            }
        }
        // Logical destination without shorthand, a destination no vCPU has (the broadcast ID
        // included) and reserved bits on an otherwise legal command.
        assert_ipi_refused(&mut device, 0, icr(NMI, 1 << 11, NO_SHORTHAND, LT_00, 0));
        for destination in [2u64, 3, 0xff] {
            device
                .write_from_cpu(0, dword(0x310), destination << 24)
                .unwrap();
            assert_ipi_refused(&mut device, 0, icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, 0));
        }
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let legal = icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, 0);
        for bit in [13u32, 16, 17, 20, 31, 32] {
            assert_ipi_refused(&mut device, 0, legal | (1 << bit));
        }
        assert_eq!(nmi_holders(&device), Vec::<usize>::new());
        // The legal command is accepted afterwards.
        device.write_from_cpu(0, dword(0x300), legal).unwrap();
        assert_eq!(nmi_holders(&device), [1]);
    }

    #[test]
    fn nmi_to_a_globally_disabled_lapic_is_refused_and_a_broadcast_is_all_or_none() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        device
            .handle_for(2)
            .unwrap()
            .set_global_enabled(false)
            .unwrap();
        device.write_from_cpu(0, dword(0x310), 2 << 24).unwrap();
        assert_ipi_refused(&mut device, 0, icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, 0));
        // All-but-self with one refusing target: no LAPIC gets the NMI.
        assert_ipi_refused(&mut device, 0, icr(NMI, PHYSICAL, OTHERS_SH, LT_00, 0));
        assert_eq!(nmi_holders(&device), Vec::<usize>::new());
        // The enabled target alone accepts it. Software disable through SVR does not refuse.
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        device.write_from_cpu(1, dword(0xf0), 0xff).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, 0))
            .unwrap();
        assert_eq!(nmi_holders(&device), [1]);
    }

    #[test]
    fn nmi_keeps_one_pending_event_and_further_ones_are_lost() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let command = icr(NMI, PHYSICAL, NO_SHORTHAND, LT_00, 0);
        for _ in 0..3 {
            device.write_from_cpu(0, dword(0x300), command).unwrap();
        }
        let target = device.handle_for(1).unwrap();
        assert!(target.take_nmi(), "one NMI is remembered");
        assert!(!target.take_nmi(), "the other two were the same event");
        assert!(!device.handle().take_nmi());
        // An INIT reset takes the latch with the rest of the LAPIC state; a plain read of the
        // pending flag does not consume it.
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        assert!(target.pending_nmi() && target.pending_nmi());
        device
            .write_from_cpu(0, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0))
            .unwrap();
        target.take_startup().unwrap();
        assert!(!target.pending_nmi());
    }

    // S6-13: INIT and SIPI wait in the LAPIC of the target.

    const INIT: u64 = 5;
    const SIPI: u64 = 6;

    fn startup_of(device: &LocalApic, cpu: usize) -> PendingStartup {
        device.handle_for(cpu).unwrap().pending_startup()
    }

    #[test]
    fn init_assert_is_pending_in_the_target_lapic_only() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        device.write_from_cpu(0, dword(0x310), 2 << 24).unwrap();
        let command = icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(
            startup_of(&device, 2),
            PendingStartup {
                init: true,
                sipi: None
            }
        );
        assert_eq!(startup_of(&device, 0), PendingStartup::default());
        assert_eq!(startup_of(&device, 1), PendingStartup::default());
        // The sender keeps the command; Delivery Status reads Idle; no IRR bit anywhere.
        assert_eq!(device.read_from_cpu(0, dword(0x300)), Ok(command));
        assert_eq!(holders(&device, 0), Vec::<usize>::new());
    }

    #[test]
    fn init_deassert_is_accepted_and_changes_no_cpu_or_lapic_state() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0))
            .unwrap();
        device
            .handle_for(1)
            .unwrap()
            .request_fixed_edge(0x61)
            .unwrap();
        let mut before = device.handle().0.lock().unwrap().clone();
        let command = icr(INIT, PHYSICAL, NO_SHORTHAND, LT_01, 0);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        // Only the ICR of the sender differs; the pending INIT stays pending and is not repeated.
        before.cpus[0].local.icr_low = command as u32;
        assert_eq!(*device.handle().0.lock().unwrap(), before);
        assert!(startup_of(&device, 1).init);
        // Deassert of an INIT that was never asserted pends nothing.
        let mut single = LocalApic::with_cpus(2).unwrap();
        single.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        single.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(startup_of(&single, 1), PendingStartup::default());
        // A destination that no vCPU has is still an error.
        single.write_from_cpu(0, dword(0x310), 2 << 24).unwrap();
        assert_ipi_refused(&mut single, 0, command);
    }

    #[test]
    fn sipi_is_pending_with_its_vector_for_every_vector_in_the_target_lapic() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        for vector in [0x00u8, 0x01, 0x08, 0x9f, 0xff] {
            let command = icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, vector);
            device.write_from_cpu(0, dword(0x300), command).unwrap();
            assert_eq!(
                startup_of(&device, 1),
                PendingStartup {
                    init: false,
                    sipi: Some(vector)
                },
                "{vector:#x}"
            );
            assert_eq!(startup_of(&device, 0), PendingStartup::default());
            // Taking it consumes it.
            device.handle_for(1).unwrap().take_startup().unwrap();
            assert_eq!(startup_of(&device, 1), PendingStartup::default());
        }
    }

    #[test]
    fn the_first_of_several_sipi_waits_and_an_init_drops_only_the_sipi_before_it() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let sipi = |vector| icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, vector);
        device.write_from_cpu(0, dword(0x300), sipi(0x08)).unwrap();
        device.write_from_cpu(0, dword(0x300), sipi(0x09)).unwrap();
        assert_eq!(startup_of(&device, 1).sipi, Some(0x08));
        // INIT: the SIPI accepted before it is dropped, the SIPI after it is kept.
        let init = icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0);
        device.write_from_cpu(0, dword(0x300), init).unwrap();
        assert_eq!(
            startup_of(&device, 1),
            PendingStartup {
                init: true,
                sipi: None
            }
        );
        device.write_from_cpu(0, dword(0x300), sipi(0x0a)).unwrap();
        // A second INIT before the first is applied is the same single event.
        device.write_from_cpu(0, dword(0x300), init).unwrap();
        assert_eq!(
            startup_of(&device, 1),
            PendingStartup {
                init: true,
                sipi: None
            }
        );
    }

    #[test]
    fn init_to_the_boot_processor_is_refused_and_sipi_to_it_is_pending() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        // Sender 1 targets itself (an application processor): INIT is pending.
        device.write_from_cpu(1, dword(0x310), 1 << 24).unwrap();
        device
            .write_from_cpu(1, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0))
            .unwrap();
        assert!(startup_of(&device, 1).init);
        // INIT assert to vCPU 0, from itself or from another vCPU: BSP INIT is deferred to
        // S6-15/S6-16, so the device refuses it and nothing changes.
        for sender in [0, 1] {
            device.write_from_cpu(sender, dword(0x310), 0).unwrap();
            assert_ipi_refused(
                &mut device,
                sender,
                icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0),
            );
            assert_eq!(startup_of(&device, 0), PendingStartup::default());
        }
        // INIT deassert and SIPI to the boot processor are accepted (SIPI is ignored by the vCPU).
        device
            .write_from_cpu(0, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_01, 0))
            .unwrap();
        device
            .write_from_cpu(
                0,
                dword(0x300),
                icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, 0x10),
            )
            .unwrap();
        assert_eq!(
            startup_of(&device, 0),
            PendingStartup {
                init: false,
                sipi: Some(0x10)
            }
        );
    }

    #[test]
    fn init_and_sipi_matrix_cells_with_any_other_encoding_are_refused() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        for level_trigger in [LT_00, LT_10, LT_01, LT_11] {
            for vector in [0u8, 1, 0x10, 0xff] {
                // INIT: only vector 0 with (1,1) assert or (0,1) deassert.
                let init = icr(INIT, PHYSICAL, NO_SHORTHAND, level_trigger, vector);
                if vector == 0 && matches!(level_trigger, LT_11 | LT_01) {
                    device.write_from_cpu(0, dword(0x300), init).unwrap();
                } else {
                    assert_ipi_refused(&mut device, 0, init);
                }
                // SIPI: only (0,0), any vector.
                let sipi = icr(SIPI, PHYSICAL, NO_SHORTHAND, level_trigger, vector);
                if level_trigger == LT_00 {
                    device.write_from_cpu(0, dword(0x300), sipi).unwrap();
                } else {
                    assert_ipi_refused(&mut device, 0, sipi);
                }
            }
        }
        // Logical destination mode and every shorthand, for both modes.
        for mode in [INIT, SIPI] {
            let level_trigger = if mode == INIT { LT_11 } else { LT_00 };
            assert_ipi_refused(
                &mut device,
                0,
                icr(mode, LOGICAL, NO_SHORTHAND, level_trigger, 0),
            );
            for shorthand in [SELF_SH, ALL_SH, OTHERS_SH] {
                assert_ipi_refused(
                    &mut device,
                    0,
                    icr(mode, PHYSICAL, shorthand, level_trigger, 0),
                );
            }
            // No vCPU has this APIC ID, including the xAPIC broadcast ID.
            for destination in [2u64, 0xff] {
                device
                    .write_from_cpu(0, dword(0x310), destination << 24)
                    .unwrap();
                assert_ipi_refused(
                    &mut device,
                    0,
                    icr(mode, PHYSICAL, NO_SHORTHAND, level_trigger, 0),
                );
            }
            device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
            // A reserved bit does not become legal.
            assert_ipi_refused(
                &mut device,
                0,
                icr(mode, PHYSICAL, NO_SHORTHAND, level_trigger, 0) | (1 << 13),
            );
        }
    }

    #[test]
    fn init_and_sipi_to_a_target_with_ia32_apic_base_en_clear_are_refused_like_fixed() {
        // Owner decision 2026-11-16: the refusal for EN=0 (IA32_APIC_BASE.EN, the global enable
        // `set_global_enabled(false)` models) covers INIT and SIPI as well as Fixed.
        let mut device = LocalApic::with_cpus(2).unwrap();
        let target = device.handle_for(1).unwrap();
        target.set_global_enabled(false).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        assert_ipi_refused(&mut device, 0, icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0));
        assert_ipi_refused(
            &mut device,
            0,
            icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, 0x08),
        );
        assert_ipi_refused(
            &mut device,
            0,
            icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61),
        );
        assert_eq!(startup_of(&device, 1), PendingStartup::default());
        // With EN set again, the same commands are accepted.
        target.set_global_enabled(true).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0))
            .unwrap();
        device
            .write_from_cpu(
                0,
                dword(0x300),
                icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, 0x08),
            )
            .unwrap();
        assert_eq!(
            startup_of(&device, 1),
            PendingStartup {
                init: true,
                sipi: Some(0x08)
            }
        );
    }

    #[test]
    fn init_deassert_to_a_target_with_ia32_apic_base_en_clear_is_accepted_without_change() {
        // MACHINE_CONTRACT.md (S6-13): with EN=0 INIT assert and SIPI are refused, but a deassert
        // changes nothing and is accepted for an existing target.
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        device
            .handle_for(1)
            .unwrap()
            .set_global_enabled(false)
            .unwrap();
        let mut before = device.handle().0.lock().unwrap().clone();
        let command = icr(INIT, PHYSICAL, NO_SHORTHAND, LT_01, 0);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        // Only the ICR of the sender differs; nothing is pending and EN stays clear.
        before.cpus[0].local.icr_low = command as u32;
        assert_eq!(*device.handle().0.lock().unwrap(), before);
        assert_eq!(startup_of(&device, 1), PendingStartup::default());
    }

    #[test]
    fn a_target_with_the_svr_software_enable_clear_accepts_init_and_sipi() {
        // Owner decision 2026-11-16 (SDM 10.4.3, from memory; 13.4.3 in SDM 093): a program
        // disable through the SVR (APIC Software Enable, bit 8, clear; here SVR = 0xFF) with EN
        // set still takes INIT and SIPI, unlike EN=0.
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(1, dword(0xf0), 0xff).unwrap();
        assert_eq!(device.read_from_cpu(1, dword(0xf0)), Ok(0xff));
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        device
            .write_from_cpu(
                0,
                dword(0x300),
                icr(SIPI, PHYSICAL, NO_SHORTHAND, LT_00, 0x08),
            )
            .unwrap();
        assert_eq!(startup_of(&device, 1).sipi, Some(0x08));
        device
            .write_from_cpu(0, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0))
            .unwrap();
        assert_eq!(
            startup_of(&device, 1),
            PendingStartup {
                init: true,
                sipi: None
            }
        );
    }

    #[test]
    fn taking_a_pending_init_puts_the_lapic_into_its_init_reset_state() {
        let (mut device, _clock) = smp_timer_device(2);
        let (cpu0, cpu1) = (device.handle(), device.handle_for(1).unwrap());
        for cpu in 0..2 {
            device.write_from_cpu(cpu, dword(0xf0), 0x1ff).unwrap();
            device.write_from_cpu(cpu, dword(0x80), 0x20).unwrap();
            device.write_from_cpu(cpu, dword(0x320), 0x40).unwrap();
            device.write_from_cpu(cpu, dword(0x380), 100).unwrap();
            // S7-32 slice M: the INIT reset masks the PMC LVT again (SDM Vol. 3A 13.4.7.3).
            device.write_from_cpu(cpu, dword(0x340), 0xfe).unwrap();
        }
        cpu0.request_fixed_edge(0x70).unwrap();
        cpu1.request_fixed_edge(0x70).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let init = icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0);
        device.write_from_cpu(0, dword(0x300), init).unwrap();
        let cpu0_before = cpu0.0.lock().unwrap().cpus[0].clone();
        // Nothing of the target changes until it takes the INIT itself.
        assert_eq!(cpu1.pending(), Some(0x70));
        assert_eq!(
            cpu1.take_startup(),
            Ok(PendingStartup {
                init: true,
                sipi: None
            })
        );
        let pair = cpu1.0.lock().unwrap();
        assert_eq!(
            pair.cpus[1].local,
            State {
                global_enabled: true,
                ..State::default()
            }
        );
        assert_eq!(pair.cpus[1].timer.as_ref().unwrap().lvt, MASK);
        assert!(pair.cpus[1].timer.as_ref().unwrap().queue.is_empty());
        // The other LAPIC is untouched, and the one taken is not taken twice.
        assert_eq!(pair.cpus[0], cpu0_before);
        drop(pair);
        assert_eq!(cpu1.take_startup(), Ok(PendingStartup::default()));
        // The APIC ID is the index, so it survives.
        assert_eq!(device.read_from_cpu(1, dword(0x20)), Ok(1 << 24));
    }

    #[test]
    fn taking_an_init_does_not_clear_the_remote_irr_of_a_level_route_to_this_lapic() {
        // Owner decision 2026-11-16: as on hardware, INIT leaves the IOAPIC's Remote IRR alone;
        // only the EOI clears it (SDM).
        let mut device = LocalApic::with_cpus(2).unwrap();
        let cpu1 = device.handle_for(1).unwrap();
        device.write_from_cpu(1, dword(0xf0), 0x1ff).unwrap();
        {
            let mut pair = cpu1.0.lock().unwrap();
            pair.route.low = LEVEL | 0x50;
            pair.route.dest[4] = 1;
            pair.route.remote_irr = true;
            pair.route.sci_remote_irr = false;
        }
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(INIT, PHYSICAL, NO_SHORTHAND, LT_11, 0))
            .unwrap();
        assert!(cpu1.0.lock().unwrap().awaits_eoi(1));
        let route_before = cpu1.0.lock().unwrap().route.clone();
        cpu1.take_startup().unwrap();
        let pair = cpu1.0.lock().unwrap();
        assert!(pair.awaits_eoi(1));
        assert!(pair.route.remote_irr);
        assert_eq!(pair.route, route_before);
    }

    #[test]
    fn an_ipi_refused_by_one_target_changes_no_lapic() {
        let mut device = LocalApic::with_cpus(3).unwrap();
        // vCPU 2 is switched off through its own handle.
        device
            .handle_for(2)
            .unwrap()
            .set_global_enabled(false)
            .unwrap();
        for shorthand in [ALL_SH, OTHERS_SH] {
            assert_ipi_refused(&mut device, 0, icr(FIXED, PHYSICAL, shorthand, LT_00, 0x61));
        }
        assert_eq!(holders(&device, 0x61), Vec::<usize>::new());
        // A request that does not reach the disabled one still goes through.
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let command = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(holders(&device, 0x61), [1]);
    }

    #[test]
    fn a_physical_destination_with_a_disabled_lapic_refuses_without_changing_state() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device
            .handle_for(1)
            .unwrap()
            .set_global_enabled(false)
            .unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        assert_ipi_refused(
            &mut device,
            0,
            icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61),
        );
        assert_eq!(holders(&device, 0x61), Vec::<usize>::new());
    }

    #[test]
    fn an_ipi_does_not_change_the_other_registers_of_any_lapic() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(1, dword(0x80), 0x30).unwrap();
        device.write_from_cpu(0, dword(0x310), 1 << 24).unwrap();
        let before = device.handle().0.lock().unwrap().clone();
        let command = icr(FIXED, PHYSICAL, NO_SHORTHAND, LT_00, 0x61);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        let mut after = device.handle().0.lock().unwrap().clone();
        // Only the IRR of vCPU 1 and the ICR of vCPU 0 differ.
        after.cpus[1].local.irr = before.cpus[1].local.irr;
        after.cpus[0].local.icr_low = before.cpus[0].local.icr_low;
        assert_eq!(after, before);
    }

    /// S7-32: a fixed IPI to a logical destination reaches every vCPU whose LDR the destination
    /// names (flat model), a lowest-priority IPI reaches one of them (the first; no arbitration is
    /// modelled), and the cluster model resolves by cluster and member (SDM Vol. 3A 13.6.2.2,
    /// 13.6.2.4). SVR bit 9 (focus processor checking disable) is stored, bit 12 is refused.
    #[test]
    fn s7_32_logical_and_lowest_priority_ipis_and_svr_bit_9() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(0, dword(0xd0), 0x01 << 24).unwrap();
        device.write_from_cpu(1, dword(0xd0), 0x02 << 24).unwrap();
        device.write_from_cpu(0, dword(0x310), 0x03 << 24).unwrap();
        let command = icr(FIXED, LOGICAL, NO_SHORTHAND, LT_00, 0x61);
        device.write_from_cpu(0, dword(0x300), command).unwrap();
        assert_eq!(holders(&device, 0x61), [0, 1]);
        device.write_from_cpu(0, dword(0x310), 0x02 << 24).unwrap();
        device
            .write_from_cpu(
                0,
                dword(0x300),
                icr(FIXED, LOGICAL, NO_SHORTHAND, LT_00, 0x62),
            )
            .unwrap();
        assert_eq!(holders(&device, 0x62), [1]);
        // Lowest priority, logical 0x03: one target.
        device.write_from_cpu(0, dword(0x310), 0x03 << 24).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(1, LOGICAL, NO_SHORTHAND, LT_00, 0x63))
            .unwrap();
        assert_eq!(holders(&device, 0x63), [0]);
        // Lowest priority, physical ID 1.
        device.write_from_cpu(0, dword(0x310), 0x01 << 24).unwrap();
        device
            .write_from_cpu(0, dword(0x300), icr(1, PHYSICAL, NO_SHORTHAND, LT_00, 0x64))
            .unwrap();
        assert_eq!(holders(&device, 0x64), [1]);
        // Cluster model: cluster 1, member bit 1 of vCPU 1.
        for (cpu, ldr) in [(0usize, 0x11u64), (1, 0x12)] {
            device
                .write_from_cpu(cpu, dword(0xe0), 0x0fff_ffff)
                .unwrap();
            device.write_from_cpu(cpu, dword(0xd0), ldr << 24).unwrap();
        }
        device.write_from_cpu(0, dword(0x310), 0x12 << 24).unwrap();
        device
            .write_from_cpu(
                0,
                dword(0x300),
                icr(FIXED, LOGICAL, NO_SHORTHAND, LT_00, 0x65),
            )
            .unwrap();
        assert_eq!(holders(&device, 0x65), [1]);
        // A logical destination that names no vCPU is still refused.
        device.write_from_cpu(0, dword(0x310), 0x21 << 24).unwrap();
        assert_ipi_refused(
            &mut device,
            0,
            icr(FIXED, LOGICAL, NO_SHORTHAND, LT_00, 0x66),
        );
        // SVR: bit 9 stored, bit 12 refused.
        device.write_from_cpu(0, dword(0xf0), 0x3ff).unwrap();
        assert_eq!(device.read_from_cpu(0, dword(0xf0)), Ok(0x3ff));
        assert_eq!(
            device.write_from_cpu(0, dword(0xf0), 0x11ff),
            Err(DeviceError::UnsupportedConfiguration)
        );
    }

    /// S7-32: MSI requests resolve a logical destination through the LDR/DFR of the vCPUs; a
    /// physical one names the APIC ID; one that names no vCPU is refused.
    #[test]
    fn s7_32_msi_requests_resolve_physical_and_logical_destinations() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        device.write_from_cpu(1, dword(0xd0), 0x04 << 24).unwrap();
        let cpu0 = device.handle();
        cpu0.request_msi_edge(0x04, true, 0x71).unwrap();
        assert_eq!(holders(&device, 0x71), [1]);
        cpu0.request_msi_edge(0, false, 0x72).unwrap();
        assert_eq!(holders(&device, 0x72), [0]);
        assert_eq!(
            cpu0.request_msi_edge(0x08, true, 0x73),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            cpu0.request_msi_edge(2, false, 0x73),
            Err(DeviceError::UnsupportedConfiguration)
        );
    }
}
