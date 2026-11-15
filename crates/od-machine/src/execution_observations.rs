use std::collections::BTreeMap;

/// Maximum distinct CPL0 RDTSC instruction addresses retained in one diagnostic run.
pub const LINUX_RDTSC_SITE_CAPACITY: usize = 128;
/// Maximum successful delivered page-fault events retained in one diagnostic run.
pub const LINUX_PAGE_FAULT_EVENT_CAPACITY: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RdtscSiteCount {
    pub rip: u64,
    pub reads: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultingPageFaultContext {
    /// Guest CR3 at the faulting instruction boundary.
    pub cr3: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rcx: u64,
    pub rsi: u64,
    pub rdi: u64,
    /// Bounded frame-pointer chain read from guest RAM before exception delivery.
    pub frame_chain: Vec<crate::ConsoleContextFrame>,
    /// Why the bounded frame-pointer walk stopped.
    pub frame_chain_end: crate::FrameChainEnd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveredPageFaultObservation {
    pub rip: u64,
    pub cr2: u64,
    pub error_code: u32,
    pub access: String,
    pub cpl: u8,
    /// Selected guest registers before exception delivery mutates the CPU state.
    pub faulting_context: FaultingPageFaultContext,
}

/// Opt-in, bounded telemetry for one Linux CPL0-set run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxExecutionObservations {
    pub rdtsc_cpl0_sites: Vec<RdtscSiteCount>,
    /// CPL0 RDTSC reads for which the set loop did not report an instruction RIP.
    pub rdtsc_cpl0_unattributed_reads: u64,
    /// CPL0 RDTSC reads at new sites after the distinct-site table reached its capacity.
    pub rdtsc_cpl0_site_overflow_reads: u64,
    pub page_faults_delivered: u64,
    pub page_fault_events: Vec<DeliveredPageFaultObservation>,
    /// Delivered #PF events after the retained event capacity was reached.
    pub page_fault_events_omitted: u64,
}

#[derive(Default)]
pub(crate) struct LinuxExecutionObservationCollector {
    rdtsc_sites: BTreeMap<u64, u64>,
    rdtsc_unattributed_reads: u64,
    rdtsc_site_overflow_reads: u64,
    page_faults_delivered: u64,
    page_fault_events: Vec<DeliveredPageFaultObservation>,
    page_fault_events_omitted: u64,
}

impl LinuxExecutionObservationCollector {
    pub(crate) fn record_cpl0_rdtsc(&mut self, rip: Option<u64>) {
        let Some(rip) = rip else {
            self.rdtsc_unattributed_reads = self.rdtsc_unattributed_reads.saturating_add(1);
            return;
        };
        if let Some(reads) = self.rdtsc_sites.get_mut(&rip) {
            *reads = reads.saturating_add(1);
        } else if self.rdtsc_sites.len() < LINUX_RDTSC_SITE_CAPACITY {
            self.rdtsc_sites.insert(rip, 1);
        } else {
            self.rdtsc_site_overflow_reads = self.rdtsc_site_overflow_reads.saturating_add(1);
        }
    }

    pub(crate) fn record_page_fault(&mut self, event: DeliveredPageFaultObservation) {
        self.page_faults_delivered = self.page_faults_delivered.saturating_add(1);
        if self.page_fault_events.len() < LINUX_PAGE_FAULT_EVENT_CAPACITY {
            self.page_fault_events.push(event);
        } else {
            self.page_fault_events_omitted = self.page_fault_events_omitted.saturating_add(1);
        }
    }

    pub(crate) fn finish(self) -> LinuxExecutionObservations {
        LinuxExecutionObservations {
            rdtsc_cpl0_sites: self
                .rdtsc_sites
                .into_iter()
                .map(|(rip, reads)| RdtscSiteCount { rip, reads })
                .collect(),
            rdtsc_cpl0_unattributed_reads: self.rdtsc_unattributed_reads,
            rdtsc_cpl0_site_overflow_reads: self.rdtsc_site_overflow_reads,
            page_faults_delivered: self.page_faults_delivered,
            page_fault_events: self.page_fault_events,
            page_fault_events_omitted: self.page_fault_events_omitted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fault(rip: u64) -> DeliveredPageFaultObservation {
        DeliveredPageFaultObservation {
            rip,
            cr2: 0xfeed_0000 + rip,
            error_code: 0x7,
            access: "Write".into(),
            cpl: 3,
            faulting_context: FaultingPageFaultContext {
                cr3: 0x1000,
                rsp: 0x2000,
                rbp: 0x3000,
                rcx: 0x4000,
                rsi: 0x5000,
                rdi: 0x6000,
                frame_chain: Vec::new(),
                frame_chain_end: crate::FrameChainEnd::ReadFailed,
            },
        }
    }

    #[test]
    fn rdtsc_sites_count_exact_rips_and_report_new_site_overflow() {
        let mut collector = LinuxExecutionObservationCollector::default();
        collector.record_cpl0_rdtsc(Some(0x2000));
        collector.record_cpl0_rdtsc(Some(0x1000));
        collector.record_cpl0_rdtsc(Some(0x2000));
        for index in 3..=LINUX_RDTSC_SITE_CAPACITY as u64 {
            collector.record_cpl0_rdtsc(Some(index * 0x1000));
        }
        collector.record_cpl0_rdtsc(Some(0xdead_0000));
        collector.record_cpl0_rdtsc(None);

        let observations = collector.finish();
        assert_eq!(
            observations.rdtsc_cpl0_sites.len(),
            LINUX_RDTSC_SITE_CAPACITY
        );
        assert_eq!(
            observations.rdtsc_cpl0_sites[0],
            RdtscSiteCount {
                rip: 0x1000,
                reads: 1
            }
        );
        assert_eq!(
            observations.rdtsc_cpl0_sites[1],
            RdtscSiteCount {
                rip: 0x2000,
                reads: 2
            }
        );
        assert_eq!(observations.rdtsc_cpl0_site_overflow_reads, 1);
        assert_eq!(observations.rdtsc_cpl0_unattributed_reads, 1);
    }

    #[test]
    fn delivered_page_faults_keep_true_total_when_event_samples_are_capped() {
        let mut collector = LinuxExecutionObservationCollector::default();
        for rip in 0..(LINUX_PAGE_FAULT_EVENT_CAPACITY as u64 + 3) {
            collector.record_page_fault(fault(rip));
        }

        let observations = collector.finish();
        assert_eq!(
            observations.page_faults_delivered,
            LINUX_PAGE_FAULT_EVENT_CAPACITY as u64 + 3
        );
        assert_eq!(
            observations.page_fault_events.len(),
            LINUX_PAGE_FAULT_EVENT_CAPACITY
        );
        assert_eq!(observations.page_fault_events_omitted, 3);
        assert_eq!(observations.page_fault_events[0], fault(0));
        assert_eq!(observations.page_fault_events.last(), Some(&fault(255)));
    }
}
