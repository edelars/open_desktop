//! Caller-driven lifecycle for the fixed Stage4FlatProbeV1 board.

use crate::{
    EventGeneration, EventQueueError, GuestPhysicalRange, LoadedStage4FlatProbe, Long64ProbeConfig,
    PagedProbeExit, PagedProbeQuantumReport, ProbeConfigError, Stage4FlatProbeLoadError,
    UartRxDispatchExit, UartRxDispatchReport, UartRxEventPump, UartRxPumpError, VirtualClock,
    VirtualClockError, load_stage4_flat_probe, load_stage4_uart_echo_probe,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, StiShadow};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagedVcpuHostState {
    Active,
    Paused,
    Stopped,
    Terminal,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PagedVcpuError {
    Image(Stage4FlatProbeLoadError),
    Paging(ProbeConfigError),
    InvalidSeed,
    NotActive(PagedVcpuHostState),
    NotPaused(PagedVcpuHostState),
    Halted,
    Queue(EventQueueError),
    Dispatch(UartRxPumpError),
    Clock(VirtualClockError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagedProbeProfile {
    Flat,
    UartEcho,
}

/// Fixed-image, synchronous owner. Time and dispatch decisions belong to the caller.
pub struct PagedVcpuSession {
    machine: LoadedStage4FlatProbe,
    profile: PagedProbeProfile,
    rflags_defined: u64,
    host: PagedVcpuHostState,
    rx: UartRxEventPump,
    clock: VirtualClock,
}

impl PagedVcpuSession {
    pub fn new(image: &[u8], rx_capacity: usize, tsc_hz: u64) -> Result<Self, PagedVcpuError> {
        Self::with_profile(image, rx_capacity, tsc_hz, PagedProbeProfile::Flat)
    }

    pub fn with_profile(
        image: &[u8],
        rx_capacity: usize,
        tsc_hz: u64,
        profile: PagedProbeProfile,
    ) -> Result<Self, PagedVcpuError> {
        let clock = VirtualClock::new(tsc_hz).map_err(PagedVcpuError::Clock)?;
        let machine = Self::prepare(image, profile)?;
        let rflags_defined = machine.rflags_defined();
        let rx = UartRxEventPump::new(rx_capacity, machine.uart());
        Ok(Self {
            machine,
            profile,
            rflags_defined,
            host: PagedVcpuHostState::Active,
            rx,
            clock,
        })
    }

    fn prepare(
        image: &[u8],
        profile: PagedProbeProfile,
    ) -> Result<LoadedStage4FlatProbe, PagedVcpuError> {
        let machine = match profile {
            PagedProbeProfile::Flat => load_stage4_flat_probe(image),
            PagedProbeProfile::UartEcho => load_stage4_uart_echo_probe(image),
        }
        .map_err(PagedVcpuError::Image)?;
        let state = machine.state();
        Long64ProbeConfig::from_cpu_state(state, machine.cr3(), machine.efer())
            .map_err(PagedVcpuError::Paging)?;
        // The loader supplies Long64/CPL0 and the exact fixed CPU profile.
        if state.cr3.is_some()
            || state.cr0 != Some(0x8001_0033)
            || state.cr4 != Some(0x20)
            || machine.efer() != 0xd00
            || state.activity != Some(CpuActivity::Running)
            || state.sti_shadow != Some(StiShadow::Clear)
            || machine.rflags_defined() & !INTEGER_FLAGS != 0
        {
            return Err(PagedVcpuError::InvalidSeed);
        }
        Ok(machine)
    }

    pub fn state(&self) -> &LongModeState {
        self.machine.state()
    }
    pub fn host_state(&self) -> PagedVcpuHostState {
        self.host
    }
    pub fn rflags_defined(&self) -> u64 {
        self.rflags_defined
    }
    /// The clock advances only on explicit request and is retained across reset.
    pub fn clock(&self) -> &VirtualClock {
        &self.clock
    }

    /// Advance while the host is active, including when the guest is halted.
    /// This does not wake the guest or dispatch events.
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), PagedVcpuError> {
        if self.host != PagedVcpuHostState::Active {
            return Err(PagedVcpuError::NotActive(self.host));
        }
        self.clock
            .advance_time_ns(delta_ns)
            .map_err(PagedVcpuError::Clock)
    }

    pub fn pending_rx(&self) -> usize {
        self.rx.len()
    }

    pub fn rx_generation(&self) -> EventGeneration {
        self.rx.generation()
    }

    pub fn next_rx_deadline(&self) -> Option<u64> {
        self.rx.next_deadline()
    }

    /// Complete and return bytes committed to the bounded polling UART TX path.
    /// Repeated calls do not return already drained bytes.
    pub fn drain_tx(&mut self) -> Vec<u8> {
        let uart = self.machine.uart();
        let mut bytes = Vec::new();
        while uart.transfer_holding_to_shift() {
            if let Some(byte) = uart.complete_transmission() {
                bytes.push(byte);
            }
        }
        bytes
    }

    pub fn pause(&mut self) -> Result<(), PagedVcpuError> {
        if self.host != PagedVcpuHostState::Active {
            return Err(PagedVcpuError::NotActive(self.host));
        }
        self.host = PagedVcpuHostState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), PagedVcpuError> {
        if self.host != PagedVcpuHostState::Paused {
            return Err(PagedVcpuError::NotPaused(self.host));
        }
        self.host = PagedVcpuHostState::Active;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.host = PagedVcpuHostState::Stopped;
    }

    /// Build and validate first; generation exhaustion leaves the owner unchanged.
    pub fn reset(&mut self, image: &[u8]) -> Result<(), PagedVcpuError> {
        let replacement = Self::prepare(image, self.profile)?;
        let flags = replacement.rflags_defined();
        self.rx
            .reset_and_retarget(self.rx.generation(), replacement.uart())
            .map_err(PagedVcpuError::Queue)?;
        self.machine = replacement;
        self.rflags_defined = flags;
        self.host = PagedVcpuHostState::Active;
        Ok(())
    }

    fn ensure_dispatchable(&self) -> Result<(), PagedVcpuError> {
        if self.host != PagedVcpuHostState::Active {
            return Err(PagedVcpuError::NotActive(self.host));
        }
        if self.machine.state().activity == Some(CpuActivity::Halted) {
            return Err(PagedVcpuError::Halted);
        }
        Ok(())
    }

    // A stale request must fail before lifecycle or queue admission checks.
    fn ensure_rx_control(&self, generation: EventGeneration) -> Result<(), PagedVcpuError> {
        if generation != self.rx.generation() {
            return Err(PagedVcpuError::Queue(EventQueueError::StaleGeneration));
        }
        self.ensure_dispatchable()
    }

    /// Schedule against the session clock's absolute nanosecond domain.
    pub fn schedule_rx(
        &mut self,
        expected_generation: EventGeneration,
        key: u64,
        deadline_ns: u64,
        value: u8,
    ) -> Result<(), PagedVcpuError> {
        self.ensure_rx_control(expected_generation)?;
        self.rx
            .schedule(expected_generation, key, deadline_ns, value)
            .map_err(PagedVcpuError::Queue)
    }

    pub fn cancel_rx(
        &mut self,
        expected_generation: EventGeneration,
        key: u64,
    ) -> Result<(), PagedVcpuError> {
        self.ensure_rx_control(expected_generation)?;
        self.rx
            .cancel(expected_generation, key)
            .map_err(PagedVcpuError::Queue)
    }

    pub fn reschedule_rx(
        &mut self,
        expected_generation: EventGeneration,
        key: u64,
        deadline_ns: u64,
    ) -> Result<(), PagedVcpuError> {
        self.ensure_rx_control(expected_generation)?;
        self.rx
            .reschedule(expected_generation, key, deadline_ns)
            .map_err(PagedVcpuError::Queue)
    }

    pub fn dispatch_rx(
        &mut self,
        budget: usize,
        stop: impl FnMut() -> bool,
    ) -> Result<UartRxDispatchReport, PagedVcpuError> {
        self.ensure_dispatchable()?;
        let report = self
            .rx
            .dispatch_due(self.rx.generation(), self.clock.now_ns(), budget, stop)
            .map_err(PagedVcpuError::Dispatch)?;
        if report.exit == UartRxDispatchExit::Stopped {
            self.stop();
        }
        Ok(report)
    }

    pub fn run(
        &mut self,
        budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<PagedProbeQuantumReport, PagedVcpuError> {
        self.ensure_dispatchable()?;
        let report =
            self.machine
                .run_with_defined(budget, self.rflags_defined, stop, committed_write);
        self.rflags_defined = report.rflags_defined;
        match report.exit {
            PagedProbeExit::Stopped => self.stop(),
            PagedProbeExit::BudgetExhausted | PagedProbeExit::Halted => {}
            _ => self.host = PagedVcpuHostState::Terminal,
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, GuestPhysicalAddress, PortAddress, STAGE4_FLAT_PROBE_IMAGE_BYTES,
        STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_UART_BASE,
    };
    use od_isa::Register;

    fn image() -> Vec<u8> {
        let mut image = vec![0; STAGE4_FLAT_PROBE_IMAGE_BYTES];
        for (gpa, value) in [
            (0x1000, 0x2003_u64),
            (0x2000, 0x3003),
            (0x3010, 0x4003),
            (0x3020, 0x5003),
            (0x3028, 0x6003),
            (0x4000, 0x7001),
            (0x5000, 0x9003 | (1 << 63)),
            (0x6000, 0xa003 | (1 << 63)),
        ] {
            let offset = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
            image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        image[0x6000..0x6005].copy_from_slice(&[0x48, 0x8b, 0x03, 0xee, 0xf4]);
        image[0x8000..0x8008].copy_from_slice(&0x41_u64.to_le_bytes());
        image
    }

    fn run(s: &mut PagedVcpuSession, n: u64) -> PagedProbeQuantumReport {
        s.run(n, || false, |_| {}).unwrap()
    }

    #[test]
    fn echo_profile_is_strict_and_reset_preserves_profile() {
        let flat = image();
        let mut echo = flat.clone();
        echo[0x6000..0x6005].copy_from_slice(&[0xec, 0xee, 0xf4, 0, 0]);
        assert!(PagedVcpuSession::new(&echo, 1, 1).is_err());
        assert!(PagedVcpuSession::with_profile(&flat, 1, 1, PagedProbeProfile::UartEcho).is_err());
        let mut session =
            PagedVcpuSession::with_profile(&echo, 1, 1, PagedProbeProfile::UartEcho).unwrap();
        session
            .schedule_rx(session.rx_generation(), 1, 0, 0x80)
            .unwrap();
        let generation = session.rx_generation();
        let state = session.state().clone();
        let clock = *session.clock();
        assert!(session.reset(&flat).is_err());
        assert_eq!(session.pending_rx(), 1);
        assert_eq!(session.rx_generation(), generation);
        assert_eq!(*session.state(), state);
        assert_eq!(*session.clock(), clock);
        session.reset(&echo).unwrap();
        assert_eq!(session.pending_rx(), 0);
        assert_eq!(session.state().rip, 0x400000);
    }

    #[test]
    fn echo_in_releases_receiver_and_tx_drain_is_once() {
        let mut echo = image();
        echo[0x6000..0x6005].copy_from_slice(&[0xec, 0xee, 0xf4, 0, 0]);
        let mut session =
            PagedVcpuSession::with_profile(&echo, 1, 1, PagedProbeProfile::UartEcho).unwrap();
        let generation = session.rx_generation();
        session.schedule_rx(generation, 1, 0, 0x80).unwrap();
        assert_eq!(session.dispatch_rx(1, || false).unwrap().delivered, 1);
        assert_eq!(run(&mut session, 1).instructions_executed, 1);
        session.schedule_rx(generation, 2, 0, 0xff).unwrap();
        assert_eq!(session.dispatch_rx(1, || false).unwrap().delivered, 1);
        assert_eq!(run(&mut session, 2).exit, PagedProbeExit::Halted);
        assert_eq!(session.drain_tx(), [0x80]);
        assert!(session.drain_tx().is_empty());
    }

    fn read_rx(s: &mut PagedVcpuSession) -> u32 {
        s.machine
            .bus_mut()
            .read_port(
                PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE),
                AccessWidth::Byte,
            )
            .unwrap()
    }

    #[test]
    fn rx_control_rejects_stale_generation_after_reset_and_key_reuse() {
        let mut s = PagedVcpuSession::new(&image(), 2, 1_000_000_000).unwrap();
        let old = s.rx_generation();
        s.advance_time_ns(9).unwrap();
        s.schedule_rx(old, 1, 3, 0x11).unwrap();
        assert!(s.reset(&[]).is_err());
        assert_eq!(s.rx_generation(), old);
        s.reset(&image()).unwrap();
        let current = s.rx_generation();
        assert_ne!(old, current);
        assert_eq!(s.clock().now_ns(), 9);
        s.schedule_rx(current, 1, 9, 0x22).unwrap();
        let stale = Err(PagedVcpuError::Queue(EventQueueError::StaleGeneration));
        assert_eq!(s.schedule_rx(old, 2, 0, 0x33), stale);
        assert_eq!(s.cancel_rx(old, 1), stale);
        assert_eq!(s.reschedule_rx(old, 1, 0), stale);
        assert_eq!(s.next_rx_deadline(), Some(9));
        assert_eq!(s.dispatch_rx(1, || false).unwrap().delivered, 1);
        assert_eq!(read_rx(&mut s), 0x22);
        s.reset(&image()).unwrap();
        assert_ne!(s.rx_generation(), current);
        assert_eq!(s.next_rx_deadline(), None);
        assert_eq!(s.cancel_rx(current, 1), stale);
    }

    #[test]
    fn rx_control_cancel_due_and_reschedule_order() {
        let mut s = PagedVcpuSession::new(&image(), 5, 1_000_000_000).unwrap();
        let g = s.rx_generation();
        for (key, deadline) in [(1, 0), (2, 20), (3, 30), (4, 10), (5, 10)] {
            s.schedule_rx(g, key, deadline, key as u8).unwrap();
        }
        s.cancel_rx(g, 1).unwrap();
        s.reschedule_rx(g, 2, 5).unwrap();
        s.reschedule_rx(g, 3, 10).unwrap();
        s.reschedule_rx(g, 4, 40).unwrap();
        s.reschedule_rx(g, 5, 10).unwrap();
        s.advance_time_ns(40).unwrap();
        for value in [2, 3, 5, 4] {
            assert_eq!(s.dispatch_rx(1, || false).unwrap().delivered, 1);
            assert_eq!(read_rx(&mut s), value);
        }
        assert_eq!(s.next_rx_deadline(), None);
        assert_eq!(
            s.dispatch_rx(1, || false).unwrap().exit,
            UartRxDispatchExit::NoEvent
        );
    }

    #[test]
    fn rx_control_handles_busy_payload_without_duplicate_delivery() {
        let mut s = PagedVcpuSession::new(&image(), 3, 1_000_000_000).unwrap();
        let g = s.rx_generation();
        for key in 1..=3 {
            s.schedule_rx(g, key, 0, key as u8).unwrap();
        }
        let report = s.dispatch_rx(3, || false).unwrap();
        assert_eq!(
            (report.delivered, report.exit),
            (1, UartRxDispatchExit::ReceiverBusy)
        );
        s.reschedule_rx(g, 2, 10).unwrap();
        assert_eq!(read_rx(&mut s), 1);
        assert_eq!(s.dispatch_rx(2, || false).unwrap().delivered, 1);
        assert_eq!(read_rx(&mut s), 3);
        s.advance_time_ns(10).unwrap();
        s.machine.uart().inject_received_byte(0xaa).unwrap();
        assert_eq!(
            s.dispatch_rx(1, || false).unwrap().exit,
            UartRxDispatchExit::ReceiverBusy
        );
        s.cancel_rx(g, 2).unwrap();
        assert_eq!(read_rx(&mut s), 0xaa);
        assert_eq!(
            s.dispatch_rx(1, || false).unwrap().exit,
            UartRxDispatchExit::NoEvent
        );
    }

    #[test]
    fn rx_control_errors_are_atomic_and_lifecycle_precedence_is_stable() {
        for mode in 0..4 {
            let mut s = PagedVcpuSession::new(&image(), 1, 1_000_000_000).unwrap();
            let stale = s.rx_generation();
            s.reset(&image()).unwrap();
            let g = s.rx_generation();
            s.schedule_rx(g, 1, 50, 0x77).unwrap();
            assert_eq!(
                s.schedule_rx(g, 1, 0, 0),
                Err(PagedVcpuError::Queue(EventQueueError::DuplicateKey))
            );
            assert_eq!(
                s.schedule_rx(g, 2, 0, 0),
                Err(PagedVcpuError::Queue(EventQueueError::Full))
            );
            assert_eq!(
                s.cancel_rx(g, 2),
                Err(PagedVcpuError::Queue(EventQueueError::NotFound))
            );
            assert_eq!(
                s.reschedule_rx(g, 2, 0),
                Err(PagedVcpuError::Queue(EventQueueError::NotFound))
            );
            let expected = match mode {
                0 => {
                    s.pause().unwrap();
                    PagedVcpuError::NotActive(PagedVcpuHostState::Paused)
                }
                1 => {
                    s.stop();
                    PagedVcpuError::NotActive(PagedVcpuHostState::Stopped)
                }
                2 => {
                    assert_eq!(run(&mut s, 3).exit, PagedProbeExit::Halted);
                    PagedVcpuError::Halted
                }
                _ => {
                    s.machine
                        .bus_mut()
                        .write_physical(
                            GuestPhysicalAddress::new(0x4000),
                            AccessWidth::Qword,
                            0,
                            |_| {},
                        )
                        .unwrap();
                    assert!(matches!(run(&mut s, 1).exit, PagedProbeExit::Fault(_)));
                    PagedVcpuError::NotActive(PagedVcpuHostState::Terminal)
                }
            };
            let before_state = s.state().clone();
            let before_clock = *s.clock();
            for result in [
                s.schedule_rx(stale, 1, 0, 0),
                s.cancel_rx(stale, 1),
                s.reschedule_rx(stale, 1, 0),
            ] {
                assert_eq!(
                    result,
                    Err(PagedVcpuError::Queue(EventQueueError::StaleGeneration))
                );
            }
            assert_eq!(s.schedule_rx(g, 1, 0, 0), Err(expected));
            let expected = if mode == 2 {
                PagedVcpuError::Halted
            } else {
                PagedVcpuError::NotActive(s.host_state())
            };
            assert_eq!(s.cancel_rx(g, 1), Err(expected));
            let expected = if mode == 2 {
                PagedVcpuError::Halted
            } else {
                PagedVcpuError::NotActive(s.host_state())
            };
            assert_eq!(s.reschedule_rx(g, 1, 0), Err(expected));
            assert_eq!(s.next_rx_deadline(), Some(50));
            assert_eq!(s.pending_rx(), 1);
            assert_eq!(*s.clock(), before_clock);
            assert_eq!(*s.state(), before_state);
            if mode == 0 {
                s.resume().unwrap();
                s.advance_time_ns(50).unwrap();
                assert_eq!(s.dispatch_rx(1, || false).unwrap().delivered, 1);
                assert_eq!(read_rx(&mut s), 0x77);
            }
        }
    }

    #[test]
    fn clock_drives_rx_and_split_advances_match() {
        let mut split = PagedVcpuSession::new(&image(), 2, 3).unwrap();
        let mut whole = PagedVcpuSession::new(&image(), 2, 3).unwrap();
        split
            .schedule_rx(split.rx_generation(), 1, 10, 0x42)
            .unwrap();
        split.advance_time_ns(4).unwrap();
        assert_eq!(split.dispatch_rx(2, || false).unwrap().delivered, 0);
        // Running and polling do not advance time.
        assert_eq!(run(&mut split, 1).exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(split.clock().now_ns(), 4);
        split.advance_time_ns(6).unwrap();
        whole.advance_time_ns(10).unwrap();
        assert_eq!(split.clock(), whole.clock());
        assert_eq!(split.dispatch_rx(2, || false).unwrap().delivered, 1);
        assert_eq!(split.pending_rx(), 0);
    }

    #[test]
    fn clock_guards_halt_reset_and_overflow() {
        assert!(matches!(
            PagedVcpuSession::new(&image(), 1, 0),
            Err(PagedVcpuError::Clock(VirtualClockError::ZeroTscFrequency))
        ));
        let mut s = PagedVcpuSession::new(&image(), 1, 1_000_000_000).unwrap();
        s.advance_time_ns(7).unwrap();
        s.pause().unwrap();
        assert_eq!(
            s.advance_time_ns(1),
            Err(PagedVcpuError::NotActive(PagedVcpuHostState::Paused))
        );
        s.resume().unwrap();
        assert_eq!(run(&mut s, 3).exit, PagedProbeExit::Halted);
        s.advance_time_ns(3).unwrap();
        assert_eq!(s.clock().now_ns(), 10);
        assert_eq!(s.state().activity, Some(CpuActivity::Halted));
        assert!(matches!(
            s.dispatch_rx(1, || false),
            Err(PagedVcpuError::Halted)
        ));
        s.stop();
        assert_eq!(
            s.advance_time_ns(1),
            Err(PagedVcpuError::NotActive(PagedVcpuHostState::Stopped))
        );
        s.reset(&image()).unwrap();
        assert_eq!(s.clock().now_ns(), 10);
        s.machine
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(0x4000),
                AccessWidth::Qword,
                0,
                |_| {},
            )
            .unwrap();
        assert!(matches!(run(&mut s, 1).exit, PagedProbeExit::Fault(_)));
        assert_eq!(
            s.advance_time_ns(1),
            Err(PagedVcpuError::NotActive(PagedVcpuHostState::Terminal))
        );
        assert_eq!(s.clock().now_ns(), 10);
        let mut overflow = PagedVcpuSession::new(&image(), 1, u64::MAX).unwrap();
        let before = *overflow.clock();
        assert_eq!(
            overflow.advance_time_ns(1_000_000_001),
            Err(PagedVcpuError::Clock(VirtualClockError::TscOverflow))
        );
        assert_eq!(*overflow.clock(), before);
        let mut time = PagedVcpuSession::new(&image(), 1, 1).unwrap();
        time.advance_time_ns(u64::MAX).unwrap();
        let before = *time.clock();
        assert_eq!(
            time.advance_time_ns(1),
            Err(PagedVcpuError::Clock(VirtualClockError::TimeOverflow))
        );
        assert_eq!(*time.clock(), before);
    }

    #[test]
    fn pause_resume_store_and_out_do_not_repeat_and_halted_stop_is_final() {
        let mut s = PagedVcpuSession::new(&image(), 3, 1_000_000_000).unwrap();
        // Exercise the quantum's existing store form; the public loader profile
        // itself admits only its fixed load/OUT/HLT image.
        s.machine
            .bus_mut()
            .write_flat_bytes(
                GuestPhysicalAddress::new(0x7000),
                &[0x48, 0x89, 0x0b, 0xee, 0xf4],
                |_| {},
            )
            .unwrap();
        s.machine.state_mut().gpr[Register::Rcx as usize] = 0x55;
        s.machine.state_mut().gpr[Register::Rax as usize] = 0x41;
        let mut writes = Vec::new();
        assert_eq!(
            s.run(1, || false, |range| writes.push(range)).unwrap().exit,
            PagedProbeExit::BudgetExhausted
        );
        assert!(!writes.is_empty());
        let committed = writes.len();
        let rip = s.state().rip;
        s.pause().unwrap();
        assert!(matches!(
            s.run(1, || false, |_| {}),
            Err(PagedVcpuError::NotActive(_))
        ));
        assert!(matches!(
            s.dispatch_rx(1, || false),
            Err(PagedVcpuError::NotActive(_))
        ));
        assert_eq!(s.state().rip, rip);
        s.resume().unwrap();
        assert_eq!(
            s.run(1, || false, |range| writes.push(range)).unwrap().exit,
            PagedProbeExit::BudgetExhausted
        );
        assert_eq!(writes.len(), committed);
        let uart = s.machine.uart();
        assert!(uart.transfer_holding_to_shift());
        assert_eq!(uart.complete_transmission(), Some(0x41));
        s.pause().unwrap();
        s.resume().unwrap();
        assert_eq!(run(&mut s, 1).exit, PagedProbeExit::Halted);
        assert_eq!(uart.complete_transmission(), None);
        assert!(matches!(
            s.dispatch_rx(1, || false),
            Err(PagedVcpuError::Halted)
        ));
        s.pause().unwrap();
        s.resume().unwrap();
        assert_eq!(s.state().activity, Some(CpuActivity::Halted));
        s.stop();
        s.stop();
        assert!(matches!(
            s.run(1, || false, |_| {}),
            Err(PagedVcpuError::NotActive(PagedVcpuHostState::Stopped))
        ));
        assert!(matches!(
            s.dispatch_rx(1, || false),
            Err(PagedVcpuError::NotActive(_))
        ));
        assert_eq!(
            s.machine
                .bus_mut()
                .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
                .unwrap(),
            0x55
        );
    }

    #[test]
    fn drain_tx_consumes_committed_output_once_after_stop() {
        let mut session = PagedVcpuSession::new(&image(), 1, 1_000_000_000).unwrap();
        let mut polls = 0;
        let report = session
            .run(
                5,
                || {
                    polls += 1;
                    polls > 2
                },
                |_| {},
            )
            .unwrap();
        assert_eq!(report.exit, PagedProbeExit::Stopped);
        assert_eq!(session.drain_tx(), vec![0x41]);
        assert!(session.drain_tx().is_empty());
        assert!(session.run(1, || false, |_| {}).is_err());
    }

    #[test]
    fn stop_before_fetch_and_after_one_instruction_is_sticky() {
        for stop_after in [0, 1] {
            let mut s = PagedVcpuSession::new(&image(), 1, 1_000_000_000).unwrap();
            let mut polls = 0;
            let report = s
                .run(
                    5,
                    || {
                        polls += 1;
                        polls > stop_after
                    },
                    |_| {},
                )
                .unwrap();
            assert_eq!(report.exit, PagedProbeExit::Stopped);
            assert_eq!(report.instructions_executed, stop_after);
            assert_eq!(s.host_state(), PagedVcpuHostState::Stopped);
            assert!(s.run(1, || false, |_| {}).is_err());
        }
    }

    #[test]
    fn reset_discards_pending_payloads_retargets_uart_and_preserves_failed_reset() {
        let mut s = PagedVcpuSession::new(&image(), 4, 1_000_000_000).unwrap();
        s.advance_time_ns(100).unwrap();
        let old_generation = s.rx.generation();
        s.schedule_rx(s.rx_generation(), 1, 5, 0x11).unwrap();
        s.schedule_rx(s.rx_generation(), 2, 100, 0x22).unwrap();
        let old_uart = s.machine.uart();
        old_uart.inject_received_byte(0x33).unwrap();
        run(&mut s, 2); // load and OUT, leaving TX holding occupied
        assert!(s.reset(&[]).is_err());
        assert_eq!(s.pending_rx(), 2);
        assert_eq!(s.host_state(), PagedVcpuHostState::Active);
        s.reset(&image()).unwrap();
        assert_eq!(s.pending_rx(), 0);
        assert_eq!(s.clock().now_ns(), 100);
        assert!(matches!(
            s.rx.dispatch_due(old_generation, 100, 1, || false),
            Err(UartRxPumpError::Queue(EventQueueError::StaleGeneration))
        ));
        assert_eq!(s.dispatch_rx(2, || false).unwrap().delivered, 0);
        s.schedule_rx(s.rx_generation(), 3, 100, 0x44).unwrap();
        assert_eq!(s.dispatch_rx(2, || false).unwrap().delivered, 1);
        let byte = s
            .machine
            .bus_mut()
            .read_port(
                PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE),
                AccessWidth::Byte,
            )
            .unwrap();
        assert_eq!(byte, 0x44);
        assert!(!s.machine.uart().transfer_holding_to_shift());
        assert!(old_uart.transfer_holding_to_shift());
        assert_eq!(old_uart.complete_transmission(), Some(0x41));
        assert_eq!(run(&mut s, 3).exit, PagedProbeExit::Halted);
    }

    #[test]
    fn reset_from_paused_stopped_halted_and_faulted() {
        for mode in 0..4 {
            let mut s = PagedVcpuSession::new(&image(), 1, 1_000_000_000).unwrap();
            match mode {
                0 => s.pause().unwrap(),
                1 => s.stop(),
                2 => {
                    assert_eq!(run(&mut s, 3).exit, PagedProbeExit::Halted);
                }
                _ => {
                    s.machine
                        .bus_mut()
                        .write_physical(
                            GuestPhysicalAddress::new(0x4000),
                            AccessWidth::Qword,
                            0,
                            |_| {},
                        )
                        .unwrap();
                    assert!(matches!(run(&mut s, 1).exit, PagedProbeExit::Fault(_)));
                    assert_eq!(s.host_state(), PagedVcpuHostState::Terminal);
                }
            }
            s.reset(&image()).unwrap();
            assert_eq!(s.host_state(), PagedVcpuHostState::Active);
            assert_eq!(run(&mut s, 3).exit, PagedProbeExit::Halted);
        }
    }
}
