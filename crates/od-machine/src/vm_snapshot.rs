//! The state of the vCPUs and of their event queues of a stopped machine, as a value in memory
//! (S6-24).
//!
//! A [`VmSnapshot`] is taken on a machine that [`SmpScheduler::stop_all`] has stopped (every vCPU
//! acknowledged, the [`SafePoint`] is the proof) and is put back into the same machine by
//! [`VmSnapshot::restore`]. The same value is taken on a machine that the parallel mode has stopped
//! ([`ParallelControl::stop_all`], one thread per vCPU, S6-34): [`VmSnapshot::take_parallel`] reads
//! the vCPUs and the Local APICs after every vCPU acknowledged, and refuses to read them otherwise
//! (S6-35). It holds
//!
//! * every vCPU: the architectural state of the processor model it runs on (`LongModeState` or
//!   the real/protected legacy model of an application processor started by SIPI), its activity
//!   (including `WaitForSipi`), the guest MSR cells, CR2, the CPUID configuration and the REP
//!   restart flag ([`crate::vcpu::Vcpu`]);
//! * every Local APIC with its pending INIT, SIPI and NMI, IRR/ISR/TMR, its timer and the
//!   timer's event queue (deadlines in virtual time and the generations), and the IOAPIC route
//!   with its Remote IRR ([`crate::apic_pair::PairState`]);
//! * the virtual time at the stop: not restored (time stands over the whole stop, SMP_CONTRACT.md
//!   9), kept as a guard: a restore into a machine whose clock has moved is refused, because every
//!   deadline in the queues is an absolute virtual time;
//! * the scheduler: the position of the quantum generator, the schedule position, the turn that
//!   was cut and the current vCPU, the counters ([`SchedulerState`]).
//!
//! It does not hold guest RAM, the devices of the board (UART, PIT, PIC, PM timer, NVMe, display),
//! the interrupt source of the board or the host's caches: those are the snapshot of `S9-24`/`S9-25`,
//! whose file format and whose barrier with input and output (`S9-28`) are not decided here. This
//! is not a snapshot of the machine: with RAM and devices that moved on, a restore gives a
//! consistent processor complex over a different memory. The restore is into the same machine,
//! not into a new one.

use crate::apic_pair::PairState;
use crate::parallel_run::{ParallelControl, StopProof};
use crate::smp_scheduler::{SafePoint, SchedulerState, SmpScheduler, StopAck};
use crate::vcpu::{BOOT_VCPU_INDEX, Vcpu};
use crate::{LoadedFirmwareBoard, VirtualClockHandle};
use std::time::{Duration, Instant};

/// How long [`VmSnapshot::take_parallel`] waits for the acknowledgement flag of one vCPU that is
/// between two of its stores in the barrier (S6-36 review, M-1): the thread lowers `safe`, reads
/// the stop and raises it again within a few instructions, so the flag comes back in microseconds;
/// two seconds cover a thread that the host has descheduled in that moment, and a flag that stays
/// down longer is a thread that does not come back, reported as [`SnapshotError::SafeFlagTimeout`].
const READ_WAIT: Duration = Duration::from_secs(2);

/// What the stopped complex of vCPUs, Local APICs and the scheduler looked like.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VmSnapshot {
    vcpus: Vec<Vcpu>,
    apic: PairState,
    /// Board identity for an in-process restore, independent of virtual time.
    clock: VirtualClockHandle,
    now_ns: u64,
    scheduler: SchedulerState,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SnapshotError {
    /// The scheduler is not stopped: [`SmpScheduler::stop_all`] has not been called, or
    /// [`SmpScheduler::resume`] has since, or the parallel run has finished.
    NotStopped,
    /// The proof is from a different stop, or its vCPU count, time or acknowledged state differs.
    StaleSafePoint,
    /// The proof and the stop are the machine's, but the acknowledgement flag of a vCPU stayed
    /// down for longer than the bounded wait of [`VmSnapshot::take_parallel`] (the thread of the
    /// vCPU, between two stores in its barrier, did not come back). Not a stale proof: the stop
    /// is in force and may be tried again.
    SafeFlagTimeout { cpu: usize },
    /// The snapshot is of a machine with another number of vCPUs.
    VcpuCount { snapshot: usize, machine: usize },
    /// The snapshot was taken from a different board.
    DifferentMachine,
    /// The clock of the machine is not where it stood at the snapshot.
    ClockMoved { snapshot_ns: u64, now_ns: u64 },
    /// The Local APICs are not those of the snapshot (the number, the timers or their clock).
    Apic,
    /// The scheduler is not the one of the snapshot.
    Scheduler,
}

impl VmSnapshot {
    /// Take the state of the stopped machine. Nothing is changed in it. `point` is the answer
    /// of [`SmpScheduler::stop_all`] for this machine: without it there is no snapshot, so a
    /// partial one (some vCPUs on a safe point and some not) cannot exist.
    pub(crate) fn take(
        board: &LoadedFirmwareBoard,
        scheduler: &SmpScheduler,
        point: &SafePoint,
    ) -> Result<Self, SnapshotError> {
        if !scheduler.paused() {
            return Err(SnapshotError::NotStopped);
        }
        let now_ns = board.clock().now_ns();
        let count = board.vcpu_count();
        if point.acks.len() != count || point.now_ns != now_ns || scheduler.vcpu_count() != count {
            return Err(SnapshotError::StaleSafePoint);
        }
        let mut vcpus = Vec::with_capacity(count);
        for (cpu, ack) in point.acks.iter().enumerate() {
            let vcpu = board.vcpu(cpu).ok_or(SnapshotError::StaleSafePoint)?;
            if !Self::holds(vcpu, ack) {
                return Err(SnapshotError::StaleSafePoint);
            }
            vcpus.push(vcpu.clone());
        }
        Ok(Self {
            vcpus,
            apic: board.local_apic().snapshot_pair(),
            clock: board.clock_handle().clone(),
            now_ns,
            scheduler: scheduler.state(),
        })
    }

    /// Whether `vcpu` stands as it acknowledged the stop.
    fn holds(vcpu: &Vcpu, ack: &StopAck) -> bool {
        match ack {
            // The two kinds of a running vCPU agree with the REP restart flag the host reads
            // them from (`SmpHost::stop_ack`): a restart point is exactly a vCPU that is to
            // skip its first boundary.
            StopAck::Boundary => {
                vcpu.activity() == od_isa::CpuActivity::Running && !vcpu.skip_first_boundary
            }
            StopAck::RepRestart => {
                vcpu.activity() == od_isa::CpuActivity::Running && vcpu.skip_first_boundary
            }
            StopAck::Asleep(activity) => {
                crate::idle_arbiter::SchedActivity::from(vcpu.activity()) == *activity
            }
        }
    }

    /// Take the state of a machine that the parallel mode has stopped (S6-35): the controller of a
    /// parallel run calls it between [`ParallelControl::stop_all`] and [`ParallelControl::resume`],
    /// with the [`StopProof`] that `stop_all` returned. The controller provides both the vCPUs and
    /// the Local APIC set of the stopped run. Nothing is changed in the
    /// machine. Refused ([`SnapshotError::NotStopped`]) when no stop is in force, and
    /// ([`SnapshotError::StaleSafePoint`]) when the proof is from a different stop or disagrees
    /// with the current stop's virtual time, vCPU count, or vCPU state. A flag that stays down is
    /// [`SnapshotError::SafeFlagTimeout`] (S6-36 review, M-1). Every vCPU has
    /// acknowledged, so no instruction is half done, nothing writes to a Local APIC but the
    /// controller itself, and what is read here is the state at one safe point, ordered after the
    /// acknowledgement of every vCPU (MACHINE_CONTRACT S6-34, rule 5).
    ///
    /// Assumption: called only by the controller of the run, between `stop_all` and `resume`;
    /// there is no concurrent `resume` (the controller is one thread and does both).
    ///
    /// `scheduler` is the scheduler of the interleaved run that goes on after a restore: the
    /// parallel mode has none, so its state is the state of power-on of that scheduler (the
    /// generator at its seed, no turn cut). Not a snapshot of the machine: no RAM, no devices,
    /// no file format (`S9-24`/`S9-25`).
    pub(crate) fn take_parallel(
        control: &ParallelControl<'_, '_>,
        proof: &StopProof,
        scheduler: &SmpScheduler,
    ) -> Result<Self, SnapshotError> {
        Self::take_parallel_within(control, proof, scheduler, READ_WAIT)
    }

    /// [`Self::take_parallel`] with the bound of the wait for an acknowledgement flag given (the
    /// tests of the timeout use a short one).
    pub(crate) fn take_parallel_within(
        control: &ParallelControl<'_, '_>,
        proof: &StopProof,
        scheduler: &SmpScheduler,
        wait: Duration,
    ) -> Result<Self, SnapshotError> {
        if !control.stopped() {
            return Err(SnapshotError::NotStopped);
        }
        // The proof must have one acknowledgement for every thread of the run (review of S6-35,
        // m-1): one more would index past the vCPUs, one less would be a partial snapshot.
        let count = control.vcpu_count();
        if !control.accepts_stop_proof(proof)
            || proof.acks.len() != count
            || control.now_ns() != proof.now_ns
            || scheduler.vcpu_count() != count
        {
            return Err(SnapshotError::StaleSafePoint);
        }
        let mut vcpus = Vec::with_capacity(count);
        for (cpu, ack) in proof.acks.iter().enumerate() {
            // A thread of the run may be, for a few instructions, between lowering its `safe` flag
            // and raising it again in its barrier even under a stop whose proof was given (S6-36
            // review, M-1: it read "not standing" before the stop was set). It holds no vCPU lock
            // and changes nothing in that moment, so the reader waits for the flag within `wait`
            // instead of failing; the flag that stays down gives `SafeFlagTimeout`, never a
            // `StaleSafePoint` (the proof is not stale, the thread is slow).
            let deadline = Instant::now() + wait;
            let vcpu = loop {
                if let Some(vcpu) = control.with_vcpu(cpu, |vcpu| vcpu.clone()) {
                    break vcpu;
                }
                if !control.stopped() {
                    return Err(SnapshotError::NotStopped);
                }
                if Instant::now() >= deadline {
                    return Err(SnapshotError::SafeFlagTimeout { cpu });
                }
                std::thread::yield_now();
            };
            if !Self::holds(&vcpu, ack) {
                return Err(SnapshotError::StaleSafePoint);
            }
            vcpus.push(vcpu);
        }
        let snapshot = Self {
            vcpus,
            apic: control
                .lapic(BOOT_VCPU_INDEX)
                .ok_or(SnapshotError::Apic)?
                .snapshot_pair(),
            clock: control.clock_handle().clone(),
            now_ns: proof.now_ns,
            scheduler: scheduler.state(),
        };
        if !control.stopped() {
            return Err(SnapshotError::NotStopped);
        }
        if !control.accepts_stop_proof(proof) {
            return Err(SnapshotError::StaleSafePoint);
        }
        Ok(snapshot)
    }

    /// The virtual time at which the machine stood.
    pub(crate) fn now_ns(&self) -> u64 {
        self.now_ns
    }

    /// The Local APICs and the route as they were.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apic(&self) -> &PairState {
        &self.apic
    }

    /// The vCPUs as they were, in index order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn vcpus(&self) -> &[Vcpu] {
        &self.vcpus
    }

    /// Put every vCPU, every Local APIC (with the event queue of its timer and the Remote IRR of
    /// the route) and the scheduler back as they were at the snapshot. The machine must be stopped
    /// and its clock must stand where it stood at the snapshot; RAM, devices and the clock are not
    /// touched. All or nothing: every refusal comes before the first change. `point` is the proof
    /// of the stop of this machine, as for [`Self::take`] (a restore into a machine nobody
    /// stopped is not made); it is not the point of the snapshot, whose own clock is checked.
    pub(crate) fn restore(
        &self,
        board: &mut LoadedFirmwareBoard,
        scheduler: &mut SmpScheduler,
        point: &SafePoint,
    ) -> Result<(), SnapshotError> {
        if !scheduler.paused() {
            return Err(SnapshotError::NotStopped);
        }
        let count = board.vcpu_count();
        if self.vcpus.len() != count {
            return Err(SnapshotError::VcpuCount {
                snapshot: self.vcpus.len(),
                machine: count,
            });
        }
        if point.acks.len() != count {
            return Err(SnapshotError::StaleSafePoint);
        }
        if scheduler.vcpu_count() != count {
            return Err(SnapshotError::Scheduler);
        }
        if &self.clock != board.clock_handle() {
            return Err(SnapshotError::DifferentMachine);
        }
        let now_ns = board.clock().now_ns();
        if now_ns != self.now_ns {
            return Err(SnapshotError::ClockMoved {
                snapshot_ns: self.now_ns,
                now_ns,
            });
        }
        board
            .local_apic()
            .restore_pair(&self.apic)
            .map_err(|_| SnapshotError::Apic)?;
        // The restore may put the vCPUs back to another translation context: the turbo engine
        // forgets what it derived from the old one.
        board.reset_turbo_engine();
        for (cpu, vcpu) in self.vcpus.iter().enumerate() {
            *board.vcpu_mut(cpu).expect("the count was checked") = vcpu.clone();
        }
        scheduler
            .set_state(self.scheduler.clone())
            .map_err(|_| SnapshotError::Scheduler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apic_pair::{LEVEL, PairState};
    use crate::clock_owner::ClockOwner;
    use crate::interleaved_harness::HZ;
    use crate::smp_scheduler::{
        BoardSmpHost, Executed, QuantumPolicy, SmpScheduler, StopError, StopRequest,
    };
    use crate::vcpu::{LegacyCpu, VcpuCpuid};
    use crate::{
        AccessWidth, BusDevice, DeviceAccess, DeviceError, FirmwareBoardConfig, IoSpace, LocalApic,
        LocalApicHandle, PciEcam, VirtualClockHandle, firmware_board::assemble_board,
    };
    use od_isa::{CpuActivity, LongModeState};

    const BSP_RIP: u64 = 0xA000;
    const BSP_EFER: u64 = 0xD01;

    fn board(vcpu_count: usize) -> LoadedFirmwareBoard {
        let config = FirmwareBoardConfig {
            vcpu_count,
            ..FirmwareBoardConfig::default()
        };
        let mut board = assemble_board(&config, PciEcam::new(), None, None)
            .unwrap()
            .into_linux_board(0x1000, 0x1000)
            .unwrap();
        board.enable_clock_owner().unwrap();
        board
    }

    fn bsp_state() -> LongModeState {
        LongModeState {
            rip: BSP_RIP,
            activity: Some(CpuActivity::Running),
            ..LongModeState::default()
        }
    }

    /// The boot processor on its entry state, the application processors from power-on.
    fn power_on(board: &mut LoadedFirmwareBoard) {
        board.reset_cpu_complex(bsp_state(), BSP_EFER).unwrap();
    }

    fn scheduler(count: usize) -> SmpScheduler {
        SmpScheduler::new(count, QuantumPolicy::Fixed(10), StopRequest::new()).unwrap()
    }

    /// Stop every vCPU of the board (nothing runs: the quantum of this host runs nothing).
    fn stop(
        board: &mut LoadedFirmwareBoard,
        scheduler: &mut SmpScheduler,
    ) -> Result<crate::smp_scheduler::SafePoint, StopError<crate::FirmwareBoardError>> {
        let mut host = BoardSmpHost::new(board, |_: &mut LoadedFirmwareBoard, _, _, _| {
            Ok(Executed::default())
        })
        .unwrap();
        scheduler.stop_all(&mut host, 2)
    }

    /// Startup of AP 1 by hand: INIT, SIPI, applied on its boundary; it is `Running` in real mode.
    fn start_ap(board: &mut LoadedFirmwareBoard) {
        {
            let apic = board.local_apic();
            let mut pair = apic.0.lock().unwrap();
            pair.queue_init(1).unwrap();
            pair.queue_sipi(1, 0x08).unwrap();
        }
        // INIT and SIPI queued in this order are applied in this order, on the AP's boundary.
        let applied = board.apply_pending_startup(1).unwrap();
        assert!(applied.init && applied.started);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Running);
    }

    /// Every kind of thing that waits in a Local APIC, in both of them: a Fixed request, an NMI,
    /// an INIT and a SIPI (the INIT of the boot processor only at the level of the pair: the
    /// device itself refuses it).
    fn dirty(board: &LoadedFirmwareBoard) {
        let apic = board.local_apic();
        apic.request_fixed_edge(0x40).unwrap();
        let mut pair = apic.0.lock().unwrap();
        pair.edge(1, 0x41).unwrap();
        pair.queue_nmi(&[0, 1]).unwrap();
        pair.queue_init(1).unwrap();
        pair.queue_sipi(0, 0x09).unwrap();
        pair.cpus[0].local.tpr = 0x20;
        pair.cpus[1].local.irr[3] |= 1 << 3;
        // S7-32 slice M: an unmasked PMC LVT (NMI) is part of the state, reset masks it.
        pair.cpus[1].local.pmc_lvt = 0x400;
    }

    fn quiet(board: &LoadedFirmwareBoard) -> bool {
        board.local_apic().snapshot_pair() == {
            let mut fresh = PairState::new(vec![Default::default(); board.vcpu_count()]);
            fresh.cpus.iter_mut().for_each(|cpu| cpu.timer = None);
            fresh
        }
    }

    #[test]
    fn the_board_reset_brings_a_running_ap_back_to_wait_for_sipi_and_clears_every_lapic() {
        let mut board = board(2);
        power_on(&mut board);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::WaitForSipi);
        start_ap(&mut board);
        board.vcpu_mut(1).unwrap().guest_lstar = 0x1234;
        dirty(&board);
        assert!(!quiet(&board));
        // The boot processor was running something else, too.
        board.vcpu_mut(0).unwrap().state.rip = 0xdead;
        board.vcpu_mut(0).unwrap().guest_efer = 1;
        power_on(&mut board);
        // The AP waits for SIPI with the real-mode reset state of power-on, whatever it did.
        let ap = board.vcpu(1).unwrap();
        assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
        assert!(matches!(ap.legacy, Some(LegacyCpu::Real(_))));
        let mut wanted = crate::vcpu::Vcpu::new_ap(1);
        wanted.set_cpuid(ap.cpuid);
        assert_eq!(ap, &wanted);
        assert_eq!(ap.cpuid, VcpuCpuid::new(ap.cpuid.profile, 1, 2));
        // The boot processor has the entry state it was given.
        let bsp = board.vcpu(0).unwrap();
        assert_eq!((bsp.state.rip, bsp.guest_efer), (BSP_RIP, BSP_EFER));
        assert_eq!(bsp.activity(), CpuActivity::Running);
        assert!(bsp.legacy.is_none());
        // Nothing waits in any Local APIC (the foreign one included): no INIT, SIPI, NMI,
        // request, TPR; the route is at reset.
        assert!(quiet(&board));
        for cpu in 0..2 {
            let apic = board.local_apic_of(cpu).unwrap();
            assert!(!apic.pending_startup().init && apic.pending_startup().sipi.is_none());
            assert!(!apic.pending_nmi());
            assert_eq!(apic.pending(), None);
        }
        // A later SIPI starts the AP again.
        board
            .local_apic()
            .0
            .lock()
            .unwrap()
            .queue_sipi(1, 0x08)
            .unwrap();
        assert!(board.apply_pending_startup(1).unwrap().started);
    }

    #[test]
    fn the_board_reset_clears_the_remote_irr_that_the_init_of_the_target_left() {
        let mut board = board(2);
        power_on(&mut board);
        start_ap(&mut board);
        let apic = board.local_apic();
        {
            // A level route to the AP: delivered, Remote IRR set until the AP's EOI.
            let mut pair = apic.0.lock().unwrap();
            pair.cpus[1].local.svr = 0x1ff;
            pair.set_dest(4, 1).unwrap();
            pair.set_low(LEVEL | 0x41).unwrap();
            pair.set_line(true).unwrap();
            assert!(pair.route.remote_irr);
        }
        // INIT of the AP (the owner's decision: it does not clear the Remote IRR, only an EOI
        // does) clears its ISR and IRR, so no EOI will ever come.
        apic.0.lock().unwrap().queue_init(1).unwrap();
        board.apply_pending_startup(1).unwrap();
        {
            let mut pair = apic.0.lock().unwrap();
            assert!(pair.route.remote_irr, "INIT leaves the Remote IRR");
            assert_eq!(pair.cpus[1].local.irr, [0; 8]);
            // ... and everything that waits for that EOI refuses, for good.
            assert_eq!(pair.reset_io(), Err(DeviceError::UnsupportedConfiguration));
            assert_eq!(
                pair.set_dest(4, 0),
                Err(DeviceError::UnsupportedConfiguration)
            );
            assert_eq!(
                pair.reset_local(1),
                Err(DeviceError::UnsupportedConfiguration)
            );
        }
        assert_eq!(
            apic.for_cpu(1).unwrap().reset(),
            Err(DeviceError::UnsupportedConfiguration)
        );
        // The board reset is not vetoed by it and clears it: the route can be reset and moved.
        power_on(&mut board);
        let mut pair = apic.0.lock().unwrap();
        assert!(!pair.route.remote_irr && !pair.route.sci_remote_irr);
        pair.reset_io().unwrap();
        pair.set_dest(4, 0).unwrap();
        pair.set_dest(4, 1).unwrap();
    }

    #[test]
    fn the_state_of_a_stopped_machine_survives_a_reset_and_comes_back_whole() {
        let mut board = board(2);
        power_on(&mut board);
        start_ap(&mut board);
        board.vcpu_mut(1).unwrap().guest_sysenter_eip = 0x77;
        board.vcpu_mut(0).unwrap().skip_first_boundary = true;
        dirty(&board);
        let mut scheduler = scheduler(2);
        let point = stop(&mut board, &mut scheduler).unwrap();
        let snapshot = VmSnapshot::take(&board, &scheduler, &point).unwrap();
        let (vcpus, pair, now) = (
            snapshot.vcpus().to_vec(),
            snapshot.apic().clone(),
            snapshot.now_ns(),
        );
        // The RAM and the devices are not in it: the UART transcript and RAM are not touched by
        // the reset below and the snapshot has no field for them.
        scheduler.reset();
        power_on(&mut board);
        assert_ne!(board.vcpu(1).unwrap(), &vcpus[1]);
        assert!(quiet(&board));
        snapshot
            .restore(&mut board, &mut scheduler, &point)
            .unwrap();
        for (cpu, wanted) in vcpus.iter().enumerate() {
            assert_eq!(board.vcpu(cpu).unwrap(), wanted, "vCPU {cpu}");
        }
        assert_eq!(board.local_apic().snapshot_pair(), pair);
        assert_eq!(board.clock().now_ns(), now);
        // What waited, waits: the INIT, the NMIs, the requests, the SIPI.
        let (apic0, apic1) = (
            board.local_apic_of(0).unwrap(),
            board.local_apic_of(1).unwrap(),
        );
        assert!(apic1.pending_startup().init);
        assert_eq!(apic0.pending_startup().sipi, Some(0x09));
        assert!(apic0.pending_nmi() && apic1.pending_nmi());
        assert_eq!(apic0.vector_state(0x40), (true, false));
        assert_eq!(apic1.vector_state(0x41), (true, false));
        assert_eq!(apic1.vector_state(0x63), (true, false));
    }

    /// A restore resets the turbo engine: the engine forgets what it derived from guest memory,
    /// including the writes to watched code pages logged before (here one to a page the test
    /// watches, made after the stop).
    #[test]
    fn a_restore_resets_the_turbo_engine() {
        let mut board = board(2);
        power_on(&mut board);
        start_ap(&mut board);
        dirty(&board);
        let mut scheduler = scheduler(2);
        let point = stop(&mut board, &mut scheduler).unwrap();
        let snapshot = VmSnapshot::take(&board, &scheduler, &point).unwrap();
        if !crate::turbo::test_arena().is_some_and(|arena| board.enable_turbo(arena)) {
            // A host without the engine: nothing to reset (on macOS/aarch64 it must exist).
            assert!(
                !cfg!(all(target_os = "macos", target_arch = "aarch64")),
                "no turbo engine"
            );
            return;
        }
        let page = crate::GuestPhysicalAddress::new(0x20_0000);
        board.bus().physical_memory().set_code_watch(page, true);
        board
            .bus_mut()
            .write_flat_bytes(page, &[0x90], |_| {})
            .unwrap();
        snapshot
            .restore(&mut board, &mut scheduler, &point)
            .unwrap();
        assert!(board.turbo_engine().is_some(), "the engine stays");
        assert_eq!(
            board.bus().physical_memory().take_code_watch_hits(),
            None,
            "the logged writes were dropped by the restore"
        );
    }

    #[test]
    fn an_ipi_sent_before_the_stop_and_not_yet_accepted_is_accepted_once_after_the_restore() {
        let mut board = board(2);
        power_on(&mut board);
        start_ap(&mut board);
        let apic = board.local_apic();
        let ap = board.local_apic_of(1).unwrap();
        {
            // vCPU 0 sent a fixed IPI to the AP (the edge that the ICR write makes, see the
            // tests of the Local APIC): in the AP's IRR, not yet accepted.
            let mut pair = apic.0.lock().unwrap();
            pair.cpus[1].local.svr = 0x1ff;
            pair.edge(1, 0x61).unwrap();
        }
        assert_eq!(ap.vector_state(0x61), (true, false));
        assert_eq!(ap.eligible(), Some(0x61));
        let mut scheduler = scheduler(2);
        let point = stop(&mut board, &mut scheduler).unwrap();
        let snapshot = VmSnapshot::take(&board, &scheduler, &point).unwrap();
        // The reset loses it; the restore brings it back, still pending and not accepted.
        scheduler.reset();
        power_on(&mut board);
        assert_eq!(ap.vector_state(0x61), (false, false));
        assert_eq!(ap.acknowledge(), None);
        snapshot
            .restore(&mut board, &mut scheduler, &point)
            .unwrap();
        assert_eq!(ap.vector_state(0x61), (true, false));
        // After the resume the AP accepts exactly that vector, and only once.
        scheduler.resume();
        assert_eq!(ap.acknowledge(), Some(0x61));
        assert_eq!(ap.vector_state(0x61), (false, true));
        assert_eq!(ap.acknowledge(), None);
        assert_eq!(board.local_apic_of(0).unwrap().acknowledge(), None);
        // Another stop and restore does not make a second one.
        let point = stop(&mut board, &mut scheduler).unwrap();
        let later = VmSnapshot::take(&board, &scheduler, &point).unwrap();
        scheduler.reset();
        power_on(&mut board);
        later.restore(&mut board, &mut scheduler, &point).unwrap();
        assert_eq!(ap.vector_state(0x61), (false, true));
        assert_eq!(ap.acknowledge(), None);
    }

    #[test]
    fn a_snapshot_is_taken_only_of_a_stopped_machine_with_its_own_proof_and_put_back_only_to_where_it_was()
     {
        let mut board = board(2);
        power_on(&mut board);
        let mut scheduler = scheduler(2);
        // Not stopped: no snapshot, even with a proof from an earlier stop.
        let point = stop(&mut board, &mut scheduler).unwrap();
        scheduler.resume();
        assert_eq!(
            VmSnapshot::take(&board, &scheduler, &point),
            Err(SnapshotError::NotStopped)
        );
        let point = stop(&mut board, &mut scheduler).unwrap();
        let snapshot = VmSnapshot::take(&board, &scheduler, &point).unwrap();
        // A vCPU that is not what it acknowledged: the proof is stale.
        board.vcpu_mut(1).unwrap().state.activity = Some(CpuActivity::Running);
        assert_eq!(
            VmSnapshot::take(&board, &scheduler, &point),
            Err(SnapshotError::StaleSafePoint)
        );
        board.vcpu_mut(1).unwrap().state.activity = Some(CpuActivity::WaitForSipi);
        // The acknowledgement is of a boundary, the vCPU now stands inside a REP (and the other
        // way round): not what was acknowledged either.
        board.vcpu_mut(0).unwrap().skip_first_boundary = true;
        assert_eq!(
            VmSnapshot::take(&board, &scheduler, &point),
            Err(SnapshotError::StaleSafePoint)
        );
        board.vcpu_mut(0).unwrap().skip_first_boundary = false;
        // The clock moved after the stop: the proof and the deadlines are stale.
        board
            .clock_owner_mut()
            .unwrap()
            .account_device_ns(5)
            .unwrap();
        assert_eq!(
            VmSnapshot::take(&board, &scheduler, &point),
            Err(SnapshotError::StaleSafePoint)
        );
        assert_eq!(
            snapshot.restore(&mut board, &mut scheduler, &point),
            Err(SnapshotError::ClockMoved {
                snapshot_ns: 0,
                now_ns: 5
            })
        );
        // Another number of vCPUs: refused before any change.
        let mut other = self::board(3);
        let mut other_scheduler = self::scheduler(3);
        let other_point = stop(&mut other, &mut other_scheduler).unwrap();
        assert_eq!(
            snapshot.restore(&mut other, &mut other_scheduler, &other_point),
            Err(SnapshotError::VcpuCount {
                snapshot: 2,
                machine: 3
            })
        );
        // A running machine takes no restore.
        scheduler.resume();
        assert_eq!(
            snapshot.restore(&mut board, &mut scheduler, &point),
            Err(SnapshotError::NotStopped)
        );
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::WaitForSipi);
    }

    #[test]
    fn a_timerless_machine_refuses_another_boards_snapshot_before_restoring_any_state() {
        use crate::idle_arbiter::SchedActivity;
        use crate::parallel_run::{ParallelConfig, ParallelFinish, run_threads};
        use od_isa::real_mode_exec::EFLAGS_IF;

        let mut source = board(2);
        let mut destination = board(2);
        power_on(&mut source);
        power_on(&mut destination);
        let bsp = source.vcpu_mut(0).unwrap();
        bsp.state.gpr[0] = 0x1234;
        bsp.state.activity = Some(CpuActivity::Halted);
        bsp.state.rflags &= !u64::from(EFLAGS_IF);
        source.local_apic().request_fixed_edge(0x40).unwrap();
        let mut source_scheduler =
            SmpScheduler::new(2, QuantumPolicy::seeded(17), StopRequest::new()).unwrap();
        let config = ParallelConfig {
            end_when_idle: false,
            ..ParallelConfig::default()
        };
        let (report, parallel_snapshot) = run_threads(
            &mut source,
            config,
            &|_| panic!("the fixture's vCPUs are asleep"),
            |control| {
                let timeout = Duration::from_secs(10);
                assert!(control.wait_until(timeout, || {
                    control.activity(0) == SchedActivity::Halted
                        && control.activity(1) == SchedActivity::WaitForSipi
                }));
                let proof = control.stop_all(timeout).unwrap();
                VmSnapshot::take_parallel(control, &proof, &source_scheduler).unwrap()
            },
        )
        .unwrap();
        assert_eq!(report.finished, ParallelFinish::Stopped);

        let mut destination_scheduler = scheduler(2);
        let source_point = stop(&mut source, &mut source_scheduler).unwrap();
        let destination_point = stop(&mut destination, &mut destination_scheduler).unwrap();
        let interleaved_snapshot =
            VmSnapshot::take(&source, &source_scheduler, &source_point).unwrap();
        let destination_before =
            VmSnapshot::take(&destination, &destination_scheduler, &destination_point).unwrap();

        for snapshot in [interleaved_snapshot, parallel_snapshot] {
            assert_eq!(snapshot.now_ns(), destination_before.now_ns());
            assert!(
                snapshot
                    .apic()
                    .cpus
                    .iter()
                    .chain(&destination_before.apic().cpus)
                    .all(|cpu| cpu.timer.is_none())
            );
            assert_ne!(snapshot.vcpus(), destination_before.vcpus());
            assert_ne!(snapshot.apic(), destination_before.apic());
            assert_ne!(snapshot.scheduler, destination_before.scheduler);
            assert_eq!(
                snapshot.restore(
                    &mut destination,
                    &mut destination_scheduler,
                    &destination_point,
                ),
                Err(SnapshotError::DifferentMachine)
            );
            assert_eq!(
                VmSnapshot::take(&destination, &destination_scheduler, &destination_point).unwrap(),
                destination_before
            );
        }
    }

    // ---- the event queue of a LAPIC timer ----

    struct Timers {
        owner: ClockOwner,
        device: LocalApic,
    }

    impl Timers {
        fn new() -> Self {
            let clock = VirtualClockHandle::new(HZ).unwrap();
            let device = LocalApic::with_shared_timers(clock.clone(), HZ, 2).unwrap();
            let apics = (0..2).map(|cpu| device.handle_for(cpu).unwrap()).collect();
            let owner = ClockOwner::new(&clock, apics).unwrap();
            let mut timers = Self { owner, device };
            for cpu in 0..2 {
                timers.write(cpu, 0xf0, 0x1ff);
            }
            timers
        }
        fn write(&mut self, cpu: usize, offset: u64, value: u64) {
            let access = DeviceAccess {
                space: IoSpace::Mmio,
                offset,
                width: AccessWidth::Dword,
            };
            self.device.write_from_cpu(cpu, access, value).unwrap();
        }
        /// LVT timer, divide-by-1 and initial count: the count is nanoseconds at `HZ` = 1 GHz.
        fn arm(&mut self, cpu: usize, lvt: u64, count: u64) {
            self.write(cpu, 0x320, lvt);
            self.write(cpu, 0x3e0, 0xb);
            self.write(cpu, 0x380, count);
        }
        fn apic(&self, cpu: usize) -> LocalApicHandle {
            self.device.handle_for(cpu).unwrap()
        }
        fn irr(&self, cpu: usize) -> [u32; 8] {
            self.apic(cpu).snapshot_pair().cpus[cpu].local.irr
        }
        /// What the guest does with a request: take it and end it.
        fn serve(&mut self, cpu: usize) -> bool {
            let taken = self.apic(cpu).acknowledge().is_some();
            if taken {
                self.write(cpu, 0xb0, 0);
            }
            taken
        }
        /// Move the clock by `ns` and dispatch every timer: what fired, per vCPU.
        fn advance(&mut self, ns: u64) -> Vec<bool> {
            self.owner.account_device_ns(ns).unwrap()
        }
    }

    const ONE_SHOT: u64 = 0x40;
    const PERIODIC_LVT: u64 = 0x41 | (1 << 17);

    #[test]
    fn a_timer_armed_before_the_stop_fires_once_at_the_same_virtual_moment_after_a_restore() {
        // Two identical machines; the second is never stopped. One-shot on vCPU 1 and periodic on
        // vCPU 0, both armed before the stop.
        let (mut stopped, mut control) = (Timers::new(), Timers::new());
        for timers in [&mut stopped, &mut control] {
            timers.arm(1, ONE_SHOT, 5_000);
            timers.arm(0, PERIODIC_LVT, 3_000);
            timers.advance(1_200);
        }
        assert_eq!(stopped.apic(1).next_timer_deadline(), Some(5_000));
        assert_eq!(stopped.apic(0).next_timer_deadline(), Some(3_000));
        // Stop, take the state of the LAPICs, reset them (the queues are emptied), put it back.
        let apic = stopped.apic(0);
        let state = apic.snapshot_pair();
        let before = (
            stopped.apic(0).next_timer_deadline(),
            stopped.apic(1).next_timer_deadline(),
        );
        apic.reset_all().unwrap();
        assert_eq!(
            (
                stopped.apic(0).next_timer_deadline(),
                stopped.apic(1).next_timer_deadline()
            ),
            (None, None)
        );
        apic.restore_pair(&state).unwrap();
        assert_eq!(
            (
                stopped.apic(0).next_timer_deadline(),
                stopped.apic(1).next_timer_deadline()
            ),
            before
        );
        assert_eq!(apic.snapshot_pair(), state);
        // Both machines go on in the same steps: the same requests at the same moments, the
        // one-shot exactly once, the periodic timer on its phase.
        let mut fired = [(0u32, Vec::new()), (0u32, Vec::new())];
        for (index, timers) in [&mut stopped, &mut control].into_iter().enumerate() {
            for _ in (1_200..=12_000).step_by(100) {
                let fires = timers.advance(100);
                let moment = timers.owner.now_ns();
                if fires[1] {
                    fired[index].0 += 1;
                    fired[index].1.push((1, moment));
                }
                if fires[0] {
                    fired[index].1.push((0, moment));
                }
                // The request of a timer that fired is there to be taken, and only then.
                assert_eq!(timers.serve(1), fires[1]);
                timers.serve(0);
            }
        }
        assert_eq!(fired[0].0, 1, "the one-shot fires exactly once");
        assert!(
            fired[0].1.contains(&(1, 5_000)),
            "at its deadline: {:?}",
            fired[0].1
        );
        assert_eq!(fired[0], fired[1], "as the machine that was never stopped");
        assert!(fired[0].1.iter().filter(|(cpu, _)| *cpu == 0).count() >= 3);
        assert_eq!(stopped.apic(1).next_timer_deadline(), None);
        assert_eq!(
            (stopped.irr(0), stopped.irr(1)),
            (control.irr(0), control.irr(1))
        );
    }

    #[test]
    fn a_restore_of_the_state_of_another_lapic_set_is_refused_and_changes_nothing() {
        let (mut one, other) = (Timers::new(), Timers::new());
        one.arm(1, ONE_SHOT, 5_000);
        let state = one.apic(0).snapshot_pair();
        let before = other.apic(0).snapshot_pair();
        // Another clock: the deadlines would be those of a time that is not this machine's.
        assert_eq!(
            other.apic(0).restore_pair(&state),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(other.apic(0).snapshot_pair(), before);
        // Another number of Local APICs, or none with a timer.
        let three = LocalApic::with_cpus(3).unwrap().handle();
        assert_eq!(
            three.restore_pair(&state),
            Err(DeviceError::UnsupportedConfiguration)
        );
    }
}
