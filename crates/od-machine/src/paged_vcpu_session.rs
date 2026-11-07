//! Caller-driven lifecycle for the fixed Stage4FlatProbeV1 board.

use crate::{
    EventQueueError, GuestPhysicalRange, LoadedStage4FlatProbe, Long64ProbeConfig, PagedProbeExit,
    PagedProbeQuantumReport, ProbeConfigError, Stage4FlatProbeLoadError, UartRxDispatchExit,
    UartRxDispatchReport, UartRxEventPump, UartRxPumpError, load_stage4_flat_probe,
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
}

/// Fixed-image, synchronous owner. Time and dispatch decisions belong to the caller.
pub struct PagedVcpuSession {
    machine: LoadedStage4FlatProbe,
    rflags_defined: u64,
    host: PagedVcpuHostState,
    rx: UartRxEventPump,
}

impl PagedVcpuSession {
    pub fn new(image: &[u8], rx_capacity: usize) -> Result<Self, PagedVcpuError> {
        let machine = Self::prepare(image)?;
        let rflags_defined = machine.rflags_defined();
        let rx = UartRxEventPump::new(rx_capacity, machine.uart());
        Ok(Self {
            machine,
            rflags_defined,
            host: PagedVcpuHostState::Active,
            rx,
        })
    }

    fn prepare(image: &[u8]) -> Result<LoadedStage4FlatProbe, PagedVcpuError> {
        let machine = load_stage4_flat_probe(image).map_err(PagedVcpuError::Image)?;
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
    pub fn pending_rx(&self) -> usize {
        self.rx.len()
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
        let replacement = Self::prepare(image)?;
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

    pub fn schedule_rx(
        &mut self,
        key: u64,
        deadline_ns: u64,
        value: u8,
    ) -> Result<(), PagedVcpuError> {
        self.ensure_dispatchable()?;
        self.rx
            .schedule(self.rx.generation(), key, deadline_ns, value)
            .map_err(PagedVcpuError::Queue)
    }

    pub fn dispatch_rx(
        &mut self,
        now_ns: u64,
        budget: usize,
        stop: impl FnMut() -> bool,
    ) -> Result<UartRxDispatchReport, PagedVcpuError> {
        self.ensure_dispatchable()?;
        let report = self
            .rx
            .dispatch_due(self.rx.generation(), now_ns, budget, stop)
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
    fn pause_resume_store_and_out_do_not_repeat_and_halted_stop_is_final() {
        let mut s = PagedVcpuSession::new(&image(), 3).unwrap();
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
            s.dispatch_rx(0, 1, || false),
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
            s.dispatch_rx(0, 1, || false),
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
            s.dispatch_rx(0, 1, || false),
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
        let mut session = PagedVcpuSession::new(&image(), 1).unwrap();
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
            let mut s = PagedVcpuSession::new(&image(), 1).unwrap();
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
        let mut s = PagedVcpuSession::new(&image(), 4).unwrap();
        let old_generation = s.rx.generation();
        s.schedule_rx(1, 5, 0x11).unwrap();
        s.schedule_rx(2, 100, 0x22).unwrap();
        let old_uart = s.machine.uart();
        old_uart.inject_received_byte(0x33).unwrap();
        run(&mut s, 2); // load and OUT, leaving TX holding occupied
        assert!(s.reset(&[]).is_err());
        assert_eq!(s.pending_rx(), 2);
        assert_eq!(s.host_state(), PagedVcpuHostState::Active);
        s.reset(&image()).unwrap();
        assert_eq!(s.pending_rx(), 0);
        assert!(matches!(
            s.rx.dispatch_due(old_generation, 100, 1, || false),
            Err(UartRxPumpError::Queue(EventQueueError::StaleGeneration))
        ));
        assert_eq!(s.dispatch_rx(100, 2, || false).unwrap().delivered, 0);
        s.schedule_rx(3, 100, 0x44).unwrap();
        assert_eq!(s.dispatch_rx(100, 2, || false).unwrap().delivered, 1);
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
            let mut s = PagedVcpuSession::new(&image(), 1).unwrap();
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
