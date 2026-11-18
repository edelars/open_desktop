use std::collections::BTreeMap;

/// Maximum distinct CPL0 RDTSC instruction addresses retained in one diagnostic run.
pub const LINUX_RDTSC_SITE_CAPACITY: usize = 128;
/// Maximum successful delivered page-fault events retained in one diagnostic run.
pub const LINUX_PAGE_FAULT_EVENT_CAPACITY: usize = 256;
pub const LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY: usize = 4;

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
    pub cs_selector: Option<u16>,
    /// Bounded frame-pointer chain read from guest RAM before exception delivery.
    pub frame_chain: Vec<crate::ConsoleContextFrame>,
    /// Why the bounded frame-pointer walk stopped.
    pub frame_chain_end: crate::FrameChainEnd,
    /// The original access walk, captured before exception delivery and its A/D effects.
    pub page_walk: Option<FaultingPageWalkContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostIretPageWalkSnapshot {
    pub event_ordinal: u64,
    pub fault_rip: u64,
    pub fault_cr2: u64,
    pub fault_access: String,
    pub fault_cpl: u8,
    pub post_iret_rip: u64,
    pub post_iret_cs: Option<u16>,
    pub post_iret_cpl: u8,
    pub post_iret_rsp: u64,
    pub post_iret_cr3: u64,
    pub entries: Vec<crate::WalkEntry>,
    pub translation_status: &'static str,
    pub translated_physical: Option<u64>,
    pub translation_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingPostIretObservation {
    pub(crate) event_ordinal: u64,
    pub(crate) fault_rip: u64,
    pub(crate) fault_cr2: u64,
    pub(crate) fault_access: String,
    pub(crate) fault_cpl: u8,
    pub(crate) fault_rsp: u64,
    pub(crate) fault_cs: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultingPageWalkContext {
    /// Raw table entries observed by the faulting access, before its A/D updates.
    pub entries: Vec<crate::WalkEntry>,
    pub reason: crate::PageFaultReason,
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
    pub post_iret_selected_ordinal: Option<u64>,
    pub post_iret_page_walk: Option<PostIretPageWalkSnapshot>,
    pub post_iret_page_walk_status: Option<&'static str>,
}

#[derive(Default)]
pub(crate) struct LinuxExecutionObservationCollector {
    rdtsc_sites: BTreeMap<u64, u64>,
    rdtsc_unattributed_reads: u64,
    rdtsc_site_overflow_reads: u64,
    page_faults_delivered: u64,
    page_fault_events: Vec<DeliveredPageFaultObservation>,
    page_fault_events_omitted: u64,
    selected_post_iret_ordinal: Option<u64>,
    pending_post_iret: Option<PendingPostIretObservation>,
    post_iret_page_walk: Option<PostIretPageWalkSnapshot>,
    post_iret_context_mismatch: bool,
    nested_iret_depth: u64,
}

impl LinuxExecutionObservationCollector {
    pub(crate) fn with_post_iret_ordinal(selected_post_iret_ordinal: Option<u64>) -> Self {
        Self {
            selected_post_iret_ordinal,
            ..Self::default()
        }
    }

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
        let ordinal = self.page_faults_delivered;
        self.page_faults_delivered = self.page_faults_delivered.saturating_add(1);
        if self.selected_post_iret_ordinal == Some(ordinal)
            && self.pending_post_iret.is_none()
            && self.post_iret_page_walk.is_none()
        {
            self.pending_post_iret = Some(PendingPostIretObservation {
                event_ordinal: ordinal,
                fault_rip: event.rip,
                fault_cr2: event.cr2,
                fault_access: event.access.clone(),
                fault_cpl: event.cpl,
                fault_rsp: event.faulting_context.rsp,
                fault_cs: event.faulting_context.cs_selector,
            });
        }
        if self.page_fault_events.len() < LINUX_PAGE_FAULT_EVENT_CAPACITY {
            self.page_fault_events.push(event);
        } else {
            self.page_fault_events_omitted = self.page_fault_events_omitted.saturating_add(1);
        }
    }

    pub(crate) fn pending_post_iret(&self) -> Option<PendingPostIretObservation> {
        self.pending_post_iret.clone()
    }

    /// Classify one retired IRETQ while the selected fault is pending. Nested returns are
    /// consumed before context comparison, because an interrupt return can coincidentally
    /// restore the selected fault context without being the selected handler's return.
    pub(crate) fn accept_post_iret_return(&mut self, context_matches: bool) -> bool {
        if self.pending_post_iret.is_none() {
            return false;
        }
        if self.nested_iret_depth != 0 {
            self.nested_iret_depth -= 1;
            return false;
        }
        if context_matches {
            return true;
        }
        self.post_iret_context_mismatch = true;
        self.pending_post_iret = None;
        false
    }

    pub(crate) fn note_nested_delivery(&mut self) {
        if self.pending_post_iret.is_some() {
            self.nested_iret_depth = self.nested_iret_depth.saturating_add(1);
        }
    }

    pub(crate) fn record_post_iret_page_walk(&mut self, snapshot: PostIretPageWalkSnapshot) {
        if self
            .pending_post_iret
            .as_ref()
            .is_some_and(|pending| pending.event_ordinal == snapshot.event_ordinal)
            && self.post_iret_page_walk.is_none()
        {
            self.post_iret_page_walk = Some(snapshot);
            self.pending_post_iret = None;
        }
    }

    pub(crate) fn finish(self) -> LinuxExecutionObservations {
        let post_iret_page_walk_status = self.selected_post_iret_ordinal.map(|_| {
            if self.post_iret_page_walk.is_some() {
                "captured"
            } else if self.post_iret_context_mismatch {
                "context_mismatch"
            } else if self.pending_post_iret.is_some() {
                "return_not_observed"
            } else {
                "event_not_seen"
            }
        });
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
            post_iret_selected_ordinal: self.selected_post_iret_ordinal,
            post_iret_page_walk: self.post_iret_page_walk,
            post_iret_page_walk_status,
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
                cs_selector: Some(0x23),
                frame_chain: Vec::new(),
                frame_chain_end: crate::FrameChainEnd::ReadFailed,
                page_walk: None,
            },
        }
    }

    fn post_iret_snapshot(pending: PendingPostIretObservation) -> PostIretPageWalkSnapshot {
        PostIretPageWalkSnapshot {
            event_ordinal: pending.event_ordinal,
            fault_rip: pending.fault_rip,
            fault_cr2: pending.fault_cr2,
            fault_access: pending.fault_access,
            fault_cpl: pending.fault_cpl,
            post_iret_rip: pending.fault_rip,
            post_iret_cs: pending.fault_cs,
            post_iret_cpl: pending.fault_cpl,
            post_iret_rsp: pending.fault_rsp,
            post_iret_cr3: 0x9000,
            entries: Vec::new(),
            translation_status: "translated",
            translated_physical: Some(0x5000),
            translation_error: None,
        }
    }

    #[test]
    fn selected_post_iret_event_stays_pending_and_is_not_replaced_by_nested_faults() {
        let mut collector = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(1));
        collector.record_page_fault(fault(0x1000));
        collector.record_page_fault(fault(0x2000));
        let selected = collector
            .pending_post_iret()
            .expect("selected event pending");
        collector.record_page_fault(fault(0x3000));
        assert_eq!(collector.pending_post_iret(), Some(selected.clone()));
        assert_eq!(selected.event_ordinal, 1);
        assert_eq!(selected.fault_rip, 0x2000);
        assert!(!collector.accept_post_iret_return(false));
        let observations = collector.finish();
        assert_eq!(
            observations.post_iret_page_walk_status,
            Some("context_mismatch")
        );
        assert_eq!(observations.post_iret_selected_ordinal, Some(1));
        assert!(observations.post_iret_page_walk.is_none());
    }

    #[test]
    fn selected_post_iret_status_distinguishes_missing_return_and_capture() {
        let missing = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(4));
        assert_eq!(
            missing.finish().post_iret_page_walk_status,
            Some("event_not_seen")
        );

        let mut unresolved = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(0));
        unresolved.record_page_fault(fault(0x4000));
        assert_eq!(
            unresolved.finish().post_iret_page_walk_status,
            Some("return_not_observed")
        );

        let mut captured = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(0));
        captured.record_page_fault(fault(0x5000));
        let selected = captured.pending_post_iret().unwrap();
        captured.record_post_iret_page_walk(post_iret_snapshot(selected));
        let observations = captured.finish();
        assert_eq!(observations.post_iret_page_walk_status, Some("captured"));
        assert_eq!(
            observations.post_iret_page_walk.unwrap().post_iret_cr3,
            0x9000
        );
    }

    #[test]
    fn default_observations_do_not_select_or_request_a_post_iret_walk() {
        let mut collector = LinuxExecutionObservationCollector::default();
        collector.record_page_fault(fault(0x6000));
        assert_eq!(collector.pending_post_iret(), None);
        let observations = collector.finish();
        assert_eq!(observations.post_iret_selected_ordinal, None);
        assert_eq!(observations.post_iret_page_walk_status, None);
        assert!(observations.post_iret_page_walk.is_none());
    }

    #[test]
    fn nested_return_mismatch_keeps_terminal_status_unresolved() {
        let mut collector = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(0));
        collector.record_page_fault(fault(0x7000));
        collector.note_nested_delivery();
        assert!(!collector.accept_post_iret_return(false));
        let observations = collector.finish();
        assert_eq!(
            observations.post_iret_page_walk_status,
            Some("return_not_observed")
        );
    }

    #[test]
    fn nested_matching_return_is_consumed_before_it_can_capture() {
        let mut collector = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(0));
        collector.record_page_fault(fault(0x8000));
        collector.note_nested_delivery();
        let pending = collector.pending_post_iret().unwrap();
        assert!(!collector.accept_post_iret_return(true));
        assert_eq!(collector.pending_post_iret(), Some(pending.clone()));
        assert!(collector.accept_post_iret_return(true));
        collector.record_post_iret_page_walk(post_iret_snapshot(pending));
        assert_eq!(
            collector.finish().post_iret_page_walk_status,
            Some("captured")
        );
    }

    #[test]
    fn terminal_context_mismatch_cannot_be_completed_by_a_later_iret() {
        let mut collector = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(0));
        collector.record_page_fault(fault(0x9000));
        let pending = collector.pending_post_iret().unwrap();
        assert!(!collector.accept_post_iret_return(false));
        assert_eq!(collector.pending_post_iret(), None);
        assert!(!collector.accept_post_iret_return(true));
        collector.record_post_iret_page_walk(post_iret_snapshot(pending));
        let observations = collector.finish();
        assert_eq!(
            observations.post_iret_page_walk_status,
            Some("context_mismatch")
        );
        assert!(observations.post_iret_page_walk.is_none());
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
