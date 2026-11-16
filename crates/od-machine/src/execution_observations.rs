use std::collections::BTreeMap;

/// Maximum distinct CPL0 RDTSC instruction addresses retained in one diagnostic run.
pub const LINUX_RDTSC_SITE_CAPACITY: usize = 128;
/// Maximum post-instruction register samples retained for one selected CPL0 RDTSC RIP.
pub const LINUX_RDTSC_SAMPLE_CAPACITY: usize = 131_072;
/// Maximum successful delivered page-fault events retained in one diagnostic run.
pub const LINUX_PAGE_FAULT_EVENT_CAPACITY: usize = 256;
pub const LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RdtscSiteCount {
    pub rip: u64,
    pub reads: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RdtscSiteSample {
    pub rip: u64,
    /// 1-based selected-RIP hit ordinal, including hits skipped by stride sampling.
    pub hit_ordinal: u64,
    /// R8 at the instruction boundary, as used by the selected kernel delay loop.
    pub requested_cycles: u64,
    /// RDI at the instruction boundary, as used by the selected kernel delay loop.
    pub start_tsc: u64,
    /// TSC value returned in EDX:EAX by this RDTSC instruction.
    pub observed_tsc: u64,
    /// Wrapping `observed_tsc - start_tsc` for direct comparison with the request.
    pub elapsed_cycles: u64,
    /// Post-instruction guest stack context for correlating the sample with its caller.
    pub rbp: u64,
    pub rsp: u64,
    pub cr3: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RdtscEntrySample {
    pub rip: u64,
    /// 1-based hit ordinal at the selected delay-function entry RIP.
    pub hit_ordinal: u64,
    pub requested_cycles: u64,
    /// TSC value returned in EDX:EAX by the entry RDTSC instruction.
    pub observed_tsc: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub cr3: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RdtscSampleWindow {
    /// Inclusive, 1-based selected-site hit ordinal.
    pub first_hit: u64,
    /// Inclusive, 1-based selected-site hit ordinal.
    pub last_hit: u64,
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
    pub rdtsc_sample_rip: Option<u64>,
    pub rdtsc_sample_stride: u64,
    pub rdtsc_sample_window: Option<RdtscSampleWindow>,
    pub rdtsc_sample_site_hits: u64,
    pub rdtsc_site_samples: Vec<RdtscSiteSample>,
    pub rdtsc_site_samples_omitted: u64,
    pub rdtsc_entry_sample_rip: Option<u64>,
    pub rdtsc_entry_sample_hits: u64,
    pub rdtsc_entry_samples: Vec<RdtscEntrySample>,
    pub rdtsc_entry_samples_omitted: u64,
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
    rdtsc_sample_rip: Option<u64>,
    rdtsc_sample_stride: u64,
    rdtsc_sample_window: Option<RdtscSampleWindow>,
    rdtsc_sample_site_hits: u64,
    rdtsc_site_samples: Vec<RdtscSiteSample>,
    rdtsc_site_samples_omitted: u64,
    rdtsc_entry_sample_rip: Option<u64>,
    rdtsc_entry_sample_hits: u64,
    rdtsc_entry_samples: Vec<RdtscEntrySample>,
    rdtsc_entry_samples_omitted: u64,
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

    pub(crate) fn with_rdtsc_sample_rip(mut self, rip: Option<u64>) -> Self {
        self.rdtsc_sample_rip = rip;
        self
    }

    pub(crate) fn with_rdtsc_sample_stride(mut self, stride: u64) -> Self {
        self.rdtsc_sample_stride = stride.max(1);
        self
    }

    pub(crate) fn with_rdtsc_sample_window(mut self, window: Option<RdtscSampleWindow>) -> Self {
        self.rdtsc_sample_window = window;
        self
    }

    pub(crate) fn with_rdtsc_entry_sample_rip(mut self, rip: Option<u64>) -> Self {
        self.rdtsc_entry_sample_rip = rip;
        self
    }

    pub(crate) fn record_rdtsc_site_sample(
        &mut self,
        rip: Option<u64>,
        state: &od_isa::LongModeState,
    ) {
        if let Some(entry_rip) = rip.filter(|rip| Some(*rip) == self.rdtsc_entry_sample_rip) {
            self.rdtsc_entry_sample_hits = self.rdtsc_entry_sample_hits.saturating_add(1);
            if self.rdtsc_entry_samples.len() < LINUX_RDTSC_SAMPLE_CAPACITY {
                use od_isa::Register;
                let rax = state.gpr[Register::Rax as usize] & u64::from(u32::MAX);
                let rdx = state.gpr[Register::Rdx as usize] & u64::from(u32::MAX);
                self.rdtsc_entry_samples.push(RdtscEntrySample {
                    rip: entry_rip,
                    hit_ordinal: self.rdtsc_entry_sample_hits,
                    requested_cycles: state.gpr[Register::R8 as usize],
                    observed_tsc: (rdx << 32) | rax,
                    rbp: state.gpr[Register::Rbp as usize],
                    rsp: state.gpr[Register::Rsp as usize],
                    cr3: state.cr3,
                });
            } else {
                self.rdtsc_entry_samples_omitted =
                    self.rdtsc_entry_samples_omitted.saturating_add(1);
            }
        }
        let Some(rip) = rip.filter(|rip| Some(*rip) == self.rdtsc_sample_rip) else {
            return;
        };
        self.rdtsc_sample_site_hits = self.rdtsc_sample_site_hits.saturating_add(1);
        let hit_ordinal = self.rdtsc_sample_site_hits;
        if self
            .rdtsc_sample_window
            .is_some_and(|window| !(window.first_hit..=window.last_hit).contains(&hit_ordinal))
        {
            return;
        }
        if hit_ordinal % self.rdtsc_sample_stride.max(1) != 0 {
            return;
        }
        if self.rdtsc_site_samples.len() >= LINUX_RDTSC_SAMPLE_CAPACITY {
            self.rdtsc_site_samples_omitted = self.rdtsc_site_samples_omitted.saturating_add(1);
            return;
        }

        use od_isa::Register;
        let rax = state.gpr[Register::Rax as usize] & u64::from(u32::MAX);
        let rdx = state.gpr[Register::Rdx as usize] & u64::from(u32::MAX);
        let observed_tsc = (rdx << 32) | rax;
        let start_tsc = state.gpr[Register::Rdi as usize];
        self.rdtsc_site_samples.push(RdtscSiteSample {
            rip,
            hit_ordinal,
            requested_cycles: state.gpr[Register::R8 as usize],
            start_tsc,
            observed_tsc,
            elapsed_cycles: observed_tsc.wrapping_sub(start_tsc),
            rbp: state.gpr[Register::Rbp as usize],
            rsp: state.gpr[Register::Rsp as usize],
            cr3: state.cr3,
        });
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
            rdtsc_sample_rip: self.rdtsc_sample_rip,
            rdtsc_sample_stride: self.rdtsc_sample_stride.max(1),
            rdtsc_sample_window: self.rdtsc_sample_window,
            rdtsc_sample_site_hits: self.rdtsc_sample_site_hits,
            rdtsc_site_samples: self.rdtsc_site_samples,
            rdtsc_site_samples_omitted: self.rdtsc_site_samples_omitted,
            rdtsc_entry_sample_rip: self.rdtsc_entry_sample_rip,
            rdtsc_entry_sample_hits: self.rdtsc_entry_sample_hits,
            rdtsc_entry_samples: self.rdtsc_entry_samples,
            rdtsc_entry_samples_omitted: self.rdtsc_entry_samples_omitted,
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
    fn selected_rdtsc_rip_keeps_post_instruction_context_and_caps_samples() {
        use od_isa::{LongModeState, Register};

        const RIP: u64 = 0xffff_ffff_8271_b34d;
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_sample_rip(Some(RIP))
            .with_rdtsc_sample_stride(1);
        let mut state = LongModeState::default();
        state.gpr[Register::R8 as usize] = 0x1234;
        state.gpr[Register::Rdi as usize] = 0xffff_ffff_ffff_ff00;
        state.gpr[Register::Rdx as usize] = 0;
        state.gpr[Register::Rax as usize] = 0x250;
        state.gpr[Register::Rbp as usize] = 0x8000;
        state.gpr[Register::Rsp as usize] = 0x7ff0;
        state.cr3 = Some(0x9000);

        collector.record_rdtsc_site_sample(Some(RIP + 2), &state);
        collector.record_rdtsc_site_sample(Some(RIP), &state);
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_sample_rip, Some(RIP));
        assert_eq!(observations.rdtsc_site_samples.len(), 1);
        assert_eq!(observations.rdtsc_site_samples_omitted, 0);
        assert_eq!(
            observations.rdtsc_site_samples[0],
            RdtscSiteSample {
                rip: RIP,
                hit_ordinal: 1,
                requested_cycles: 0x1234,
                start_tsc: 0xffff_ffff_ffff_ff00,
                observed_tsc: 0x250,
                elapsed_cycles: 0x350,
                rbp: 0x8000,
                rsp: 0x7ff0,
                cr3: Some(0x9000),
            }
        );

        let mut capped = LinuxExecutionObservationCollector::default()
            .with_rdtsc_sample_rip(Some(RIP))
            .with_rdtsc_sample_stride(1);
        for _ in 0..=LINUX_RDTSC_SAMPLE_CAPACITY {
            capped.record_rdtsc_site_sample(Some(RIP), &state);
        }
        let observations = capped.finish();
        assert_eq!(
            observations.rdtsc_site_samples.len(),
            LINUX_RDTSC_SAMPLE_CAPACITY
        );
        assert_eq!(observations.rdtsc_site_samples_omitted, 1);
        assert_eq!(
            observations.rdtsc_sample_site_hits,
            LINUX_RDTSC_SAMPLE_CAPACITY as u64 + 1
        );
    }

    #[test]
    fn selected_rdtsc_entry_rip_keeps_every_hit_and_reports_cap_omissions() {
        use od_isa::{LongModeState, Register};

        const RIP: u64 = 0xffff_ffff_8271_b31b;
        let mut collector =
            LinuxExecutionObservationCollector::default().with_rdtsc_entry_sample_rip(Some(RIP));
        let mut state = LongModeState::default();
        state.gpr[Register::R8 as usize] = 0x1234;
        state.gpr[Register::Rax as usize] = 0x5678;
        state.gpr[Register::Rdx as usize] = 0x9abc;
        state.gpr[Register::Rbp as usize] = 0x8000;
        state.gpr[Register::Rsp as usize] = 0x7ff0;
        state.cr3 = Some(0x9000);

        collector.record_rdtsc_site_sample(Some(RIP + 2), &state);
        collector.record_rdtsc_site_sample(Some(RIP), &state);
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_entry_sample_rip, Some(RIP));
        assert_eq!(observations.rdtsc_entry_sample_hits, 1);
        assert_eq!(observations.rdtsc_entry_samples_omitted, 0);
        assert_eq!(
            observations.rdtsc_entry_samples[0],
            RdtscEntrySample {
                rip: RIP,
                hit_ordinal: 1,
                requested_cycles: 0x1234,
                observed_tsc: 0x9abc_0000_5678,
                rbp: 0x8000,
                rsp: 0x7ff0,
                cr3: Some(0x9000),
            }
        );

        let mut capped =
            LinuxExecutionObservationCollector::default().with_rdtsc_entry_sample_rip(Some(RIP));
        for _ in 0..=LINUX_RDTSC_SAMPLE_CAPACITY {
            capped.record_rdtsc_site_sample(Some(RIP), &state);
        }
        let observations = capped.finish();
        assert_eq!(
            observations.rdtsc_entry_samples.len(),
            LINUX_RDTSC_SAMPLE_CAPACITY
        );
        assert_eq!(observations.rdtsc_entry_samples_omitted, 1);
        assert_eq!(
            observations.rdtsc_entry_sample_hits,
            LINUX_RDTSC_SAMPLE_CAPACITY as u64 + 1
        );
    }

    #[test]
    fn selected_rdtsc_rip_samples_each_stride_hit_and_tracks_all_hits() {
        use od_isa::LongModeState;
        const RIP: u64 = 0xffff_ffff_8271_b34d;
        let state = LongModeState::default();
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_sample_rip(Some(RIP))
            .with_rdtsc_sample_stride(3);
        for _ in 0..8 {
            collector.record_rdtsc_site_sample(Some(RIP), &state);
        }
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_sample_stride, 3);
        assert_eq!(observations.rdtsc_sample_site_hits, 8);
        assert_eq!(
            observations
                .rdtsc_site_samples
                .iter()
                .map(|sample| sample.hit_ordinal)
                .collect::<Vec<_>>(),
            vec![3, 6]
        );
        assert_eq!(observations.rdtsc_site_samples_omitted, 0);
    }

    #[test]
    fn selected_rdtsc_window_keeps_global_stride_phase_and_total_hits() {
        use od_isa::LongModeState;
        const RIP: u64 = 0xffff_ffff_8271_b34d;
        let state = LongModeState::default();
        let window = RdtscSampleWindow {
            first_hit: 2,
            last_hit: 8,
        };
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_sample_rip(Some(RIP))
            .with_rdtsc_sample_stride(3)
            .with_rdtsc_sample_window(Some(window));
        for _ in 0..10 {
            collector.record_rdtsc_site_sample(Some(RIP), &state);
        }
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_sample_window, Some(window));
        assert_eq!(observations.rdtsc_sample_site_hits, 10);
        assert_eq!(
            observations
                .rdtsc_site_samples
                .iter()
                .map(|sample| sample.hit_ordinal)
                .collect::<Vec<_>>(),
            vec![3, 6]
        );
        assert_eq!(observations.rdtsc_site_samples_omitted, 0);
    }

    #[test]
    fn window_beyond_execution_prefix_keeps_entry_stream_and_has_no_omissions() {
        use od_isa::LongModeState;
        const POLL_RIP: u64 = 0xffff_ffff_8271_b34d;
        const ENTRY_RIP: u64 = 0xffff_ffff_8271_b31b;
        let state = LongModeState::default();
        let window = RdtscSampleWindow {
            first_hit: 20,
            last_hit: 25,
        };
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_sample_rip(Some(POLL_RIP))
            .with_rdtsc_sample_window(Some(window))
            .with_rdtsc_entry_sample_rip(Some(ENTRY_RIP));
        for _ in 0..10 {
            collector.record_rdtsc_site_sample(Some(ENTRY_RIP), &state);
            collector.record_rdtsc_site_sample(Some(POLL_RIP), &state);
        }
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_sample_site_hits, 10);
        assert_eq!(observations.rdtsc_site_samples.len(), 0);
        assert_eq!(observations.rdtsc_site_samples_omitted, 0);
        assert_eq!(observations.rdtsc_entry_sample_hits, 10);
        assert_eq!(observations.rdtsc_entry_samples.len(), 10);
        assert_eq!(observations.rdtsc_entry_samples_omitted, 0);
    }

    #[test]
    fn strided_rdtsc_sample_cap_counts_only_omitted_candidates() {
        use od_isa::LongModeState;
        const RIP: u64 = 0xffff_ffff_8271_b34d;
        let state = LongModeState::default();
        let stride = 2;
        let hits = (LINUX_RDTSC_SAMPLE_CAPACITY as u64 + 5) * stride;
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_sample_rip(Some(RIP))
            .with_rdtsc_sample_stride(stride);
        for _ in 0..hits {
            collector.record_rdtsc_site_sample(Some(RIP), &state);
        }
        let observations = collector.finish();
        assert_eq!(
            observations.rdtsc_site_samples.len(),
            LINUX_RDTSC_SAMPLE_CAPACITY
        );
        assert_eq!(observations.rdtsc_sample_site_hits, hits);
        assert_eq!(observations.rdtsc_site_samples_omitted, 5);
        assert_eq!(
            observations.rdtsc_site_samples.last().unwrap().hit_ordinal,
            (LINUX_RDTSC_SAMPLE_CAPACITY as u64) * stride
        );
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
