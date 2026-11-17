use crate::clock_owner::NS_PER_RETIRED;
use std::collections::BTreeMap;

/// Maximum distinct CPL0 RDTSC instruction addresses retained in one diagnostic run.
pub const LINUX_RDTSC_SITE_CAPACITY: usize = 128;
/// Maximum post-instruction register samples retained for one selected CPL0 RDTSC RIP.
pub const LINUX_RDTSC_SAMPLE_CAPACITY: usize = 131_072;
/// Number of entry-site RDTSC callbacks for which the exact pre-charge clock ledger is retained.
pub const LINUX_RDTSC_CLOCK_TRACE_CAPACITY: usize = 2;
/// Maximum ordered RDTSC prefix events retained for the opt-in sentinel trace.
pub const LINUX_RDTSC_PREFIX_CAPACITY: usize = 131_072;
/// Hard cap for the one selected instruction-path quantum.
pub const LINUX_INSTRUCTION_PATH_TRACE_CAPACITY: usize = 2_000_000;
pub const LINUX_INSTRUCTION_PATH_TRACE_QUANTUM: u64 = 45_215;
pub const LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE: u64 = 1_203_244_482;
pub const LINUX_INSTRUCTION_PATH_TRACE_START_HITS: u64 = 8_382;
pub const LINUX_INSTRUCTION_PATH_TRACE_END_HIT: u64 = 8_383;
/// Linux delay_tsc entry instruction used to end the diagnostic prefix.
pub const LINUX_RDTSC_PREFIX_SENTINEL_RIP: u64 = 0xffff_ffff_8271_b31b;
/// Maximum successful delivered page-fault events retained in one diagnostic run.
pub const LINUX_PAGE_FAULT_EVENT_CAPACITY: usize = 256;
pub const LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY: usize = 4;
pub const LINUX_ELF_STACK_COPY_PROFILE: &str = "s5-07-elf-stack-platform-copy-v1";
pub const LINUX_ELF_STACK_COPY_RIP: u64 = 0xffff_ffff_8199_9bd4;
pub const LINUX_ELF_STACK_COPY_PROBE_CAPACITY: usize = 4;
pub const LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ElfStackCopyStatus {
    #[default]
    NotRequested,
    AwaitingTarget,
    Captured,
    GateMismatch,
    ReadError,
    RunEndedBeforeTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElfStackCopyProbe {
    pub name: String,
    pub address: u64,
    pub page_base: u64,
    pub access: String,
    pub cpl: u8,
    pub entries: Vec<crate::WalkEntry>,
    /// `translated`, `page_fault`, or `error`.
    pub outcome: String,
    pub translated_physical: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElfStackCopySnapshot {
    pub profile: &'static str,
    pub status: ElfStackCopyStatus,
    pub rip: Option<u64>,
    pub cr3: Option<u64>,
    pub cs_selector: Option<u16>,
    pub cpl: Option<u8>,
    pub r15: Option<u64>,
    pub r13: Option<u64>,
    pub rdi: Option<u64>,
    pub rsi: Option<u64>,
    pub rdx: Option<u64>,
    pub bprm_p_address: Option<u64>,
    pub bprm_p: Option<u64>,
    pub aligned_stack_pointer: Option<u64>,
    pub copy_destination: Option<u64>,
    pub copy_length: Option<u64>,
    pub instruction_bytes: Option<Vec<u8>>,
    pub mismatch_reasons: Vec<String>,
    pub probes: Vec<ElfStackCopyProbe>,
    pub error: Option<String>,
}

impl ElfStackCopySnapshot {
    pub(crate) fn empty(status: ElfStackCopyStatus) -> Self {
        Self {
            profile: LINUX_ELF_STACK_COPY_PROFILE,
            status,
            rip: None,
            cr3: None,
            cs_selector: None,
            cpl: None,
            r15: None,
            r13: None,
            rdi: None,
            rsi: None,
            rdx: None,
            bprm_p_address: None,
            bprm_p: None,
            aligned_stack_pointer: None,
            copy_destination: None,
            copy_length: None,
            instruction_bytes: None,
            mismatch_reasons: Vec::new(),
            probes: Vec::new(),
            error: None,
        }
    }
}

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

/// Clock-owner state observed inside the RDTSC callback, before the current quantum is charged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RdtscClockReadLedger {
    pub initial_now_ns: u64,
    pub now_ns: u64,
    pub tsc: u64,
    pub tsc_hz: u64,
    pub retired_instructions_charged: u64,
    pub pm_timer_ns_charged: u64,
    pub pit_ns_charged: u64,
    pub rdtsc_ns_charged: u64,
    pub idle_ns_advanced: u64,
    pub quantum_ordinal: u64,
    pub irqs_delivered: u64,
}

/// Post-report instruction counts for the quantum containing the captured RDTSC callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RdtscClockReadCompletion {
    pub quantum_instructions_executed: u64,
    pub retired_instructions_through_rdtsc: u64,
}

/// Bounded, exact-callback ledger for the first two selected entry-site reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RdtscClockTraceSample {
    pub rip: u64,
    pub hit_ordinal: u64,
    pub initial_now_ns: u64,
    pub now_ns: u64,
    pub callback_tsc: u64,
    pub tsc_hz: u64,
    pub retired_instructions_charged: u64,
    pub retired_ns_charged: Option<u64>,
    /// Instructions in the quantum containing this callback, reported after execution returns.
    pub quantum_instructions_executed: u64,
    /// Cumulative retired instructions through the RDTSC that invoked the callback.
    pub retired_instructions_through_rdtsc: u64,
    pub pm_timer_ns_charged: u64,
    pub pit_ns_charged: u64,
    pub rdtsc_ns_charged: u64,
    pub idle_ns_advanced: u64,
    pub quantum_ordinal: u64,
    pub irqs_delivered: u64,
    /// `None` means the known charged components exceeded the observed elapsed clock or overflowed.
    pub unaccounted_elapsed_ns: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RdtscPrefixSample {
    pub ordinal: u64,
    pub rip: u64,
    pub cpl: Option<u8>,
    pub cr3: Option<u64>,
    pub rsp: u64,
    pub rbp: u64,
    pub clock: RdtscClockTraceSample,
    pub page_faults_delivered_through: u64,
    pub general_protection_faults_delivered_through: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionPathTraceStatus {
    NotRequested,
    AwaitingStart,
    Captured,
    GateMismatch,
    MissedStart,
    MissedEnd,
    CapacityExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionPathTraceHeader {
    pub rip: u64,
    pub rflags: u64,
    pub gpr: [u64; 16],
    pub cr3: Option<u64>,
    pub cs_selector: Option<u16>,
    pub frame_chain: Vec<crate::ConsoleContextFrame>,
    pub frame_chain_end: crate::FrameChainEnd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionPathRegisterValue {
    pub register: u8,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionPathControlFlow {
    /// `branch`, `jump`, `call`, `return`, `indirect_jump`, or `indirect_call`.
    pub kind: &'static str,
    pub condition: Option<u8>,
    pub taken: bool,
    pub rflags_before: u64,
    pub rflags_after: u64,
    pub destination_registers: [Option<InstructionPathRegisterValue>; 2],
    pub destination: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionPathTraceRecord {
    pub rip: u64,
    pub next_rip: u64,
    pub control_flow: Option<InstructionPathControlFlow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionPathTrace {
    pub requested: bool,
    pub status: InstructionPathTraceStatus,
    pub quantum_ordinal: Option<u64>,
    pub retired_instructions_charged: Option<u64>,
    pub rdtsc_cpl0_hits_before_dispatch: Option<u64>,
    pub header: Option<InstructionPathTraceHeader>,
    pub records: Vec<InstructionPathTraceRecord>,
    pub omitted_records: u64,
    pub quantum_instructions_executed: Option<u64>,
    pub rdtsc_cpl0_hits_after_quantum: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RdtscPrefixStatus {
    #[default]
    NotRequested,
    SentinelSeen,
    CapacityExhausted,
    RunEndedBeforeSentinel,
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
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
    pub rdtsc_entry_clock_trace_samples: Vec<RdtscClockTraceSample>,
    pub rdtsc_entry_clock_trace_missing: u64,
    pub rdtsc_prefix_sentinel_rip: Option<u64>,
    pub rdtsc_prefix_samples: Vec<RdtscPrefixSample>,
    pub rdtsc_prefix_event_count: u64,
    pub rdtsc_prefix_missing: u64,
    pub rdtsc_prefix_status: RdtscPrefixStatus,
    pub instruction_path_trace: Option<InstructionPathTrace>,
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
    pub elf_stack_copy: Option<ElfStackCopySnapshot>,
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
    rdtsc_entry_clock_trace_samples: Vec<RdtscClockTraceSample>,
    rdtsc_entry_clock_trace_missing: u64,
    rdtsc_prefix_sentinel_rip: Option<u64>,
    rdtsc_prefix_samples: Vec<RdtscPrefixSample>,
    rdtsc_prefix_event_ordinal: u64,
    rdtsc_prefix_missing: u64,
    rdtsc_prefix_status: RdtscPrefixStatus,
    instruction_path_trace: Option<InstructionPathTrace>,
    rdtsc_sites: BTreeMap<u64, u64>,
    rdtsc_cpl0_hits: u64,
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
    elf_stack_copy: Option<ElfStackCopySnapshot>,
}

impl LinuxExecutionObservationCollector {
    pub(crate) fn with_elf_stack_copy(mut self, requested: bool) -> Self {
        self.elf_stack_copy =
            requested.then(|| ElfStackCopySnapshot::empty(ElfStackCopyStatus::AwaitingTarget));
        self
    }

    pub(crate) fn wants_elf_stack_copy(&self) -> bool {
        self.elf_stack_copy
            .as_ref()
            .is_some_and(|snapshot| snapshot.status == ElfStackCopyStatus::AwaitingTarget)
    }

    pub(crate) fn record_elf_stack_copy(&mut self, snapshot: ElfStackCopySnapshot) {
        if self.wants_elf_stack_copy() {
            self.elf_stack_copy = Some(snapshot);
        }
    }

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

    pub(crate) fn with_rdtsc_prefix_sentinel(mut self, sentinel_rip: Option<u64>) -> Self {
        self.rdtsc_prefix_sentinel_rip = sentinel_rip;
        self.rdtsc_prefix_status = if sentinel_rip.is_some() {
            RdtscPrefixStatus::RunEndedBeforeSentinel
        } else {
            RdtscPrefixStatus::NotRequested
        };
        self
    }

    pub(crate) fn with_instruction_path_trace(mut self, requested: bool) -> Self {
        self.instruction_path_trace = requested.then(|| InstructionPathTrace {
            requested: true,
            status: InstructionPathTraceStatus::AwaitingStart,
            quantum_ordinal: None,
            retired_instructions_charged: None,
            rdtsc_cpl0_hits_before_dispatch: None,
            header: None,
            records: Vec::new(),
            omitted_records: 0,
            quantum_instructions_executed: None,
            rdtsc_cpl0_hits_after_quantum: None,
        });
        self
    }

    /// Called immediately before dispatching each board quantum. The gate is deliberately
    /// specific to the recorded S5-07 divergence and does not change the dispatch budget.
    pub(crate) fn begin_instruction_path_quantum(
        &mut self,
        quantum_ordinal: u64,
        retired_instructions_charged: u64,
    ) {
        let Some(trace) = self.instruction_path_trace.as_mut() else {
            return;
        };
        if trace.status != InstructionPathTraceStatus::AwaitingStart {
            return;
        }
        if quantum_ordinal < LINUX_INSTRUCTION_PATH_TRACE_QUANTUM {
            return;
        }
        trace.quantum_ordinal = Some(quantum_ordinal);
        trace.retired_instructions_charged = Some(retired_instructions_charged);
        trace.rdtsc_cpl0_hits_before_dispatch = Some(self.rdtsc_cpl0_hits);
        if quantum_ordinal != LINUX_INSTRUCTION_PATH_TRACE_QUANTUM
            || retired_instructions_charged != LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE
            || self.rdtsc_cpl0_hits != LINUX_INSTRUCTION_PATH_TRACE_START_HITS
        {
            trace.status = InstructionPathTraceStatus::GateMismatch;
            return;
        }
        trace.status = InstructionPathTraceStatus::AwaitingStart;
        trace.records.clear();
    }

    pub(crate) fn wants_instruction_path_trace(&self) -> bool {
        self.instruction_path_trace.as_ref().is_some_and(|trace| {
            trace.requested
                && trace.status == InstructionPathTraceStatus::AwaitingStart
                && trace.quantum_ordinal == Some(LINUX_INSTRUCTION_PATH_TRACE_QUANTUM)
                && trace.retired_instructions_charged
                    == Some(LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE)
                && trace.rdtsc_cpl0_hits_before_dispatch
                    == Some(LINUX_INSTRUCTION_PATH_TRACE_START_HITS)
        })
    }

    pub(crate) fn needs_instruction_path_header(&self) -> bool {
        self.wants_instruction_path_trace()
            && self
                .instruction_path_trace
                .as_ref()
                .is_some_and(|trace| trace.header.is_none())
    }

    pub(crate) fn begin_instruction_path_header(
        &mut self,
        state: &od_isa::LongModeState,
        frame_chain: Vec<crate::ConsoleContextFrame>,
        frame_chain_end: crate::FrameChainEnd,
    ) {
        if !self.wants_instruction_path_trace() {
            return;
        }
        if let Some(trace) = self.instruction_path_trace.as_mut() {
            trace.header = Some(InstructionPathTraceHeader {
                rip: state.rip,
                rflags: state.rflags,
                gpr: state.gpr,
                cr3: state.cr3,
                cs_selector: state.cs_selector,
                frame_chain,
                frame_chain_end,
            });
        }
    }

    pub(crate) fn record_instruction_path_retirement(
        &mut self,
        record: InstructionPathTraceRecord,
    ) {
        if !self.wants_instruction_path_trace() {
            return;
        }
        let Some(trace) = self.instruction_path_trace.as_mut() else {
            return;
        };
        if trace.records.len() >= LINUX_INSTRUCTION_PATH_TRACE_CAPACITY {
            trace.omitted_records = trace.omitted_records.saturating_add(1);
            trace.status = InstructionPathTraceStatus::CapacityExhausted;
            return;
        }
        trace.records.push(record);
    }

    pub(crate) fn complete_instruction_path_quantum(
        &mut self,
        quantum_ordinal: u64,
        quantum_instructions_executed: u64,
    ) {
        let Some(trace) = self.instruction_path_trace.as_mut() else {
            return;
        };
        if trace.quantum_ordinal != Some(quantum_ordinal)
            || trace.status == InstructionPathTraceStatus::GateMismatch
        {
            return;
        }
        trace.quantum_instructions_executed = Some(quantum_instructions_executed);
        trace.rdtsc_cpl0_hits_after_quantum = Some(self.rdtsc_cpl0_hits);
        if trace.status == InstructionPathTraceStatus::CapacityExhausted {
            return;
        }
        if trace.header.is_none() {
            trace.status = InstructionPathTraceStatus::MissedStart;
        } else if self.rdtsc_cpl0_hits != LINUX_INSTRUCTION_PATH_TRACE_END_HIT {
            trace.status = InstructionPathTraceStatus::MissedEnd;
        } else if trace.records.len() as u64 != quantum_instructions_executed {
            trace.status = InstructionPathTraceStatus::MissedEnd;
        } else {
            trace.status = InstructionPathTraceStatus::Captured;
        }
    }

    pub(crate) fn finish_instruction_path_trace(&mut self) {
        if let Some(trace) = self.instruction_path_trace.as_mut()
            && trace.status == InstructionPathTraceStatus::AwaitingStart
        {
            trace.status = if trace.header.is_some() {
                InstructionPathTraceStatus::MissedEnd
            } else {
                InstructionPathTraceStatus::MissedStart
            };
        }
    }

    pub(crate) fn wants_rdtsc_prefix(&self) -> bool {
        self.rdtsc_prefix_sentinel_rip.is_some()
            && self.rdtsc_prefix_status == RdtscPrefixStatus::RunEndedBeforeSentinel
            && self.rdtsc_prefix_event_ordinal < LINUX_RDTSC_PREFIX_CAPACITY as u64
    }

    pub(crate) fn wants_any_rdtsc_clock_trace(&self) -> bool {
        self.wants_rdtsc_prefix() || self.wants_rdtsc_clock_trace()
    }

    pub(crate) fn wants_rdtsc_clock_trace(&self) -> bool {
        self.rdtsc_entry_sample_rip.is_some()
            && self
                .rdtsc_entry_clock_trace_samples
                .len()
                .saturating_add(self.rdtsc_entry_clock_trace_missing as usize)
                < LINUX_RDTSC_CLOCK_TRACE_CAPACITY
    }

    pub(crate) fn build_rdtsc_clock_trace_if_active<T>(
        &self,
        build: impl FnOnce() -> T,
    ) -> Option<T> {
        self.wants_any_rdtsc_clock_trace().then(build)
    }

    pub(crate) fn record_rdtsc_site_sample(
        &mut self,
        rip: Option<u64>,
        state: &od_isa::LongModeState,
        clock_ledger: Option<RdtscClockReadLedger>,
    ) {
        if let Some(entry_rip) = rip.filter(|rip| Some(*rip) == self.rdtsc_entry_sample_rip) {
            self.rdtsc_entry_sample_hits = self.rdtsc_entry_sample_hits.saturating_add(1);
            let hit_ordinal = self.rdtsc_entry_sample_hits;
            if hit_ordinal <= LINUX_RDTSC_CLOCK_TRACE_CAPACITY as u64 {
                if let Some(ledger) = clock_ledger {
                    let retired_ns_charged = ledger
                        .retired_instructions_charged
                        .checked_mul(NS_PER_RETIRED);
                    let charged_ns = retired_ns_charged.and_then(|ns| {
                        ns.checked_add(ledger.pm_timer_ns_charged)
                            .and_then(|ns| ns.checked_add(ledger.pit_ns_charged))
                            .and_then(|ns| ns.checked_add(ledger.rdtsc_ns_charged))
                            .and_then(|ns| ns.checked_add(ledger.idle_ns_advanced))
                    });
                    let clock_elapsed_ns = ledger.now_ns.checked_sub(ledger.initial_now_ns);
                    self.rdtsc_entry_clock_trace_samples
                        .push(RdtscClockTraceSample {
                            rip: entry_rip,
                            hit_ordinal,
                            initial_now_ns: ledger.initial_now_ns,
                            now_ns: ledger.now_ns,
                            callback_tsc: ledger.tsc,
                            tsc_hz: ledger.tsc_hz,
                            retired_instructions_charged: ledger.retired_instructions_charged,
                            retired_ns_charged,
                            quantum_instructions_executed: 0,
                            retired_instructions_through_rdtsc: 0,
                            pm_timer_ns_charged: ledger.pm_timer_ns_charged,
                            pit_ns_charged: ledger.pit_ns_charged,
                            rdtsc_ns_charged: ledger.rdtsc_ns_charged,
                            idle_ns_advanced: ledger.idle_ns_advanced,
                            quantum_ordinal: ledger.quantum_ordinal,
                            irqs_delivered: ledger.irqs_delivered,
                            unaccounted_elapsed_ns: clock_elapsed_ns
                                .zip(charged_ns)
                                .and_then(|(elapsed, charged)| elapsed.checked_sub(charged)),
                        });
                } else {
                    self.rdtsc_entry_clock_trace_missing =
                        self.rdtsc_entry_clock_trace_missing.saturating_add(1);
                }
            }
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

    pub(crate) fn record_rdtsc_prefix(
        &mut self,
        rip: Option<u64>,
        state: &od_isa::LongModeState,
        ledger: Option<RdtscClockReadLedger>,
        page_faults_delivered: u64,
        general_protection_faults_delivered: u64,
    ) -> Option<u64> {
        if !self.wants_rdtsc_prefix() {
            return None;
        }
        self.rdtsc_prefix_event_ordinal = self.rdtsc_prefix_event_ordinal.saturating_add(1);
        let ordinal = self.rdtsc_prefix_event_ordinal;
        let sentinel = rip.is_some_and(|rip| self.rdtsc_prefix_sentinel_rip == Some(rip));
        if let (Some(rip), Some(ledger)) = (rip, ledger) {
            let retired_ns_charged = ledger
                .retired_instructions_charged
                .checked_mul(NS_PER_RETIRED);
            let charged_ns = retired_ns_charged.and_then(|ns| {
                ns.checked_add(ledger.pm_timer_ns_charged)
                    .and_then(|ns| ns.checked_add(ledger.pit_ns_charged))
                    .and_then(|ns| ns.checked_add(ledger.rdtsc_ns_charged))
                    .and_then(|ns| ns.checked_add(ledger.idle_ns_advanced))
            });
            let clock_elapsed_ns = ledger.now_ns.checked_sub(ledger.initial_now_ns);
            let clock = RdtscClockTraceSample {
                rip,
                hit_ordinal: ordinal,
                initial_now_ns: ledger.initial_now_ns,
                now_ns: ledger.now_ns,
                callback_tsc: ledger.tsc,
                tsc_hz: ledger.tsc_hz,
                retired_instructions_charged: ledger.retired_instructions_charged,
                retired_ns_charged,
                quantum_instructions_executed: 0,
                retired_instructions_through_rdtsc: 0,
                pm_timer_ns_charged: ledger.pm_timer_ns_charged,
                pit_ns_charged: ledger.pit_ns_charged,
                rdtsc_ns_charged: ledger.rdtsc_ns_charged,
                idle_ns_advanced: ledger.idle_ns_advanced,
                quantum_ordinal: ledger.quantum_ordinal,
                irqs_delivered: ledger.irqs_delivered,
                unaccounted_elapsed_ns: clock_elapsed_ns
                    .zip(charged_ns)
                    .and_then(|(elapsed, charged)| elapsed.checked_sub(charged)),
            };
            self.rdtsc_prefix_samples.push(RdtscPrefixSample {
                ordinal,
                rip,
                cpl: state.cs_selector.map(|selector| (selector & 3) as u8),
                cr3: state.cr3,
                rsp: state.gpr[od_isa::Register::Rsp as usize],
                rbp: state.gpr[od_isa::Register::Rbp as usize],
                clock,
                page_faults_delivered_through: page_faults_delivered,
                general_protection_faults_delivered_through: general_protection_faults_delivered,
            });
        } else {
            self.rdtsc_prefix_missing = self.rdtsc_prefix_missing.saturating_add(1);
        }
        if sentinel {
            self.rdtsc_prefix_status = RdtscPrefixStatus::SentinelSeen;
        } else if ordinal == LINUX_RDTSC_PREFIX_CAPACITY as u64 {
            self.rdtsc_prefix_status = RdtscPrefixStatus::CapacityExhausted;
        }
        Some(ordinal)
    }

    pub(crate) fn complete_rdtsc_prefix(
        &mut self,
        ordinal: Option<u64>,
        completion: RdtscClockReadCompletion,
    ) {
        let Some(ordinal) = ordinal else { return };
        let Some(sample) = self
            .rdtsc_prefix_samples
            .last_mut()
            .filter(|sample| sample.ordinal == ordinal)
        else {
            return;
        };
        if sample
            .clock
            .retired_instructions_charged
            .checked_add(completion.quantum_instructions_executed)
            != Some(completion.retired_instructions_through_rdtsc)
        {
            self.rdtsc_prefix_samples.pop();
            self.rdtsc_prefix_missing = self.rdtsc_prefix_missing.saturating_add(1);
            return;
        }
        sample.clock.quantum_instructions_executed = completion.quantum_instructions_executed;
        sample.clock.retired_instructions_through_rdtsc =
            completion.retired_instructions_through_rdtsc;
    }

    /// Completes the selected callback snapshot after the containing execution quantum returns.
    pub(crate) fn complete_rdtsc_clock_trace(
        &mut self,
        rip: Option<u64>,
        completion: RdtscClockReadCompletion,
    ) {
        let Some(entry_rip) = rip.filter(|rip| Some(*rip) == self.rdtsc_entry_sample_rip) else {
            return;
        };
        let hit_ordinal = self.rdtsc_entry_sample_hits;
        if hit_ordinal > LINUX_RDTSC_CLOCK_TRACE_CAPACITY as u64 {
            return;
        }
        let Some(sample) = self
            .rdtsc_entry_clock_trace_samples
            .last_mut()
            .filter(|sample| sample.rip == entry_rip && sample.hit_ordinal == hit_ordinal)
        else {
            return;
        };
        if sample
            .retired_instructions_charged
            .checked_add(completion.quantum_instructions_executed)
            != Some(completion.retired_instructions_through_rdtsc)
        {
            self.rdtsc_entry_clock_trace_samples.pop();
            self.rdtsc_entry_clock_trace_missing =
                self.rdtsc_entry_clock_trace_missing.saturating_add(1);
            return;
        }
        sample.quantum_instructions_executed = completion.quantum_instructions_executed;
        sample.retired_instructions_through_rdtsc = completion.retired_instructions_through_rdtsc;
    }

    pub(crate) fn record_cpl0_rdtsc(&mut self, rip: Option<u64>) {
        self.rdtsc_cpl0_hits = self.rdtsc_cpl0_hits.saturating_add(1);
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
        let cpl0_rdtsc_total = self.rdtsc_sites.values().copied().fold(
            self.rdtsc_unattributed_reads
                .saturating_add(self.rdtsc_site_overflow_reads),
            u64::saturating_add,
        );
        let rdtsc_prefix_missing =
            if self.rdtsc_prefix_status == RdtscPrefixStatus::CapacityExhausted {
                cpl0_rdtsc_total.saturating_sub(self.rdtsc_prefix_samples.len() as u64)
            } else {
                self.rdtsc_prefix_missing
            };
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
            rdtsc_entry_clock_trace_samples: self.rdtsc_entry_clock_trace_samples,
            rdtsc_entry_clock_trace_missing: self.rdtsc_entry_clock_trace_missing,
            rdtsc_prefix_sentinel_rip: self.rdtsc_prefix_sentinel_rip,
            rdtsc_prefix_samples: self.rdtsc_prefix_samples,
            rdtsc_prefix_event_count: self.rdtsc_prefix_event_ordinal,
            rdtsc_prefix_missing,
            rdtsc_prefix_status: self.rdtsc_prefix_status,
            instruction_path_trace: self.instruction_path_trace,
            rdtsc_cpl0_unattributed_reads: self.rdtsc_unattributed_reads,
            rdtsc_cpl0_site_overflow_reads: self.rdtsc_site_overflow_reads,
            page_faults_delivered: self.page_faults_delivered,
            page_fault_events: self.page_fault_events,
            page_fault_events_omitted: self.page_fault_events_omitted,
            post_iret_selected_ordinal: self.selected_post_iret_ordinal,
            post_iret_page_walk: self.post_iret_page_walk,
            post_iret_page_walk_status,
            elf_stack_copy: self.elf_stack_copy.map(|mut snapshot| {
                if snapshot.status == ElfStackCopyStatus::AwaitingTarget {
                    snapshot.status = ElfStackCopyStatus::RunEndedBeforeTarget;
                }
                snapshot
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gated_instruction_trace_collector() -> LinuxExecutionObservationCollector {
        let mut collector =
            LinuxExecutionObservationCollector::default().with_instruction_path_trace(true);
        for _ in 0..LINUX_INSTRUCTION_PATH_TRACE_START_HITS {
            collector.record_cpl0_rdtsc(Some(LINUX_RDTSC_PREFIX_SENTINEL_RIP));
        }
        collector
    }

    #[test]
    fn elf_stack_copy_collector_requests_only_its_first_snapshot() {
        let mut collector = LinuxExecutionObservationCollector::default().with_elf_stack_copy(true);
        assert!(collector.wants_elf_stack_copy());
        let mut captured = ElfStackCopySnapshot::empty(ElfStackCopyStatus::Captured);
        captured.rip = Some(LINUX_ELF_STACK_COPY_RIP);
        collector.record_elf_stack_copy(captured);
        assert!(!collector.wants_elf_stack_copy());
    }

    fn trace_header() -> InstructionPathTraceHeader {
        InstructionPathTraceHeader {
            rip: 0x1000,
            rflags: 0x202,
            gpr: [0; 16],
            cr3: Some(0x3000),
            cs_selector: Some(8),
            frame_chain: Vec::new(),
            frame_chain_end: crate::FrameChainEnd::NullFramePointer,
        }
    }

    #[test]
    fn instruction_path_trace_starts_only_after_the_exact_quantum_gate() {
        let mut collector = gated_instruction_trace_collector();
        collector.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM - 1,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        assert!(!collector.wants_instruction_path_trace());
        collector.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        assert!(collector.wants_instruction_path_trace());
        assert_eq!(
            collector
                .instruction_path_trace
                .as_ref()
                .unwrap()
                .rdtsc_cpl0_hits_before_dispatch,
            Some(LINUX_INSTRUCTION_PATH_TRACE_START_HITS)
        );

        let mut mismatch = gated_instruction_trace_collector();
        mismatch.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE + 1,
        );
        assert!(!mismatch.wants_instruction_path_trace());
        assert_eq!(
            mismatch.instruction_path_trace.unwrap().status,
            InstructionPathTraceStatus::GateMismatch
        );

        let mut hit_mismatch =
            LinuxExecutionObservationCollector::default().with_instruction_path_trace(true);
        for _ in 0..(LINUX_INSTRUCTION_PATH_TRACE_START_HITS - 1) {
            hit_mismatch.record_cpl0_rdtsc(Some(LINUX_RDTSC_PREFIX_SENTINEL_RIP));
        }
        hit_mismatch.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        assert_eq!(
            hit_mismatch.instruction_path_trace.unwrap().status,
            InstructionPathTraceStatus::GateMismatch
        );
    }

    #[test]
    fn instruction_path_trace_keeps_record_order_and_branch_metadata() {
        let mut collector = gated_instruction_trace_collector();
        collector.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        collector.instruction_path_trace.as_mut().unwrap().header = Some(trace_header());
        let flow = InstructionPathControlFlow {
            kind: "branch",
            condition: Some(4),
            taken: true,
            rflags_before: 0x242,
            rflags_after: 0x202,
            destination_registers: [None, None],
            destination: 0x2000,
        };
        collector.record_instruction_path_retirement(InstructionPathTraceRecord {
            rip: 0x1000,
            next_rip: 0x2000,
            control_flow: Some(flow),
        });
        collector.record_instruction_path_retirement(InstructionPathTraceRecord {
            rip: 0x2000,
            next_rip: 0x2001,
            control_flow: None,
        });
        let records = &collector.instruction_path_trace.as_ref().unwrap().records;
        assert_eq!((records[0].rip, records[0].next_rip), (0x1000, 0x2000));
        assert_eq!((records[1].rip, records[1].next_rip), (0x2000, 0x2001));
        assert_eq!(records[0].control_flow.unwrap(), flow);
        assert_eq!(records[1].control_flow, None);
        collector.record_cpl0_rdtsc(Some(LINUX_RDTSC_PREFIX_SENTINEL_RIP));
        collector.complete_instruction_path_quantum(LINUX_INSTRUCTION_PATH_TRACE_QUANTUM, 2);
        let trace = collector.instruction_path_trace.as_ref().unwrap();
        assert_eq!(trace.status, InstructionPathTraceStatus::Captured);
        assert_eq!(
            trace.rdtsc_cpl0_hits_after_quantum,
            Some(LINUX_INSTRUCTION_PATH_TRACE_END_HIT)
        );
    }

    #[test]
    fn instruction_path_trace_overflow_and_missing_boundaries_are_incomplete() {
        let mut overflow = gated_instruction_trace_collector();
        overflow.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        let trace = overflow.instruction_path_trace.as_mut().unwrap();
        trace.records.resize(
            LINUX_INSTRUCTION_PATH_TRACE_CAPACITY,
            InstructionPathTraceRecord {
                rip: 0x1000,
                next_rip: 0x1001,
                control_flow: None,
            },
        );
        overflow.record_instruction_path_retirement(InstructionPathTraceRecord {
            rip: 0x1001,
            next_rip: 0x1002,
            control_flow: None,
        });
        assert_eq!(
            overflow.instruction_path_trace.as_ref().unwrap().status,
            InstructionPathTraceStatus::CapacityExhausted
        );
        assert_eq!(
            overflow
                .instruction_path_trace
                .as_ref()
                .unwrap()
                .omitted_records,
            1
        );

        let mut missed_end = gated_instruction_trace_collector();
        missed_end.begin_instruction_path_quantum(
            LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        missed_end.instruction_path_trace.as_mut().unwrap().header = Some(trace_header());
        missed_end.complete_instruction_path_quantum(LINUX_INSTRUCTION_PATH_TRACE_QUANTUM, 0);
        assert_eq!(
            missed_end.instruction_path_trace.as_ref().unwrap().status,
            InstructionPathTraceStatus::MissedEnd
        );

        let mut missed_start = gated_instruction_trace_collector();
        missed_start.finish_instruction_path_trace();
        assert_eq!(
            missed_start.instruction_path_trace.unwrap().status,
            InstructionPathTraceStatus::MissedStart
        );
    }

    #[test]
    fn instruction_path_trace_is_a_no_op_when_not_requested() {
        let mut collector = LinuxExecutionObservationCollector::default();
        collector.begin_instruction_path_quantum(1, 0);
        collector.record_instruction_path_retirement(InstructionPathTraceRecord {
            rip: 1,
            next_rip: 2,
            control_flow: None,
        });
        collector.complete_instruction_path_quantum(1, 1);
        collector.finish_instruction_path_trace();
        assert_eq!(collector.finish().instruction_path_trace, None);
    }

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

        collector.record_rdtsc_site_sample(Some(RIP + 2), &state, None);
        collector.record_rdtsc_site_sample(Some(RIP), &state, None);
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
            capped.record_rdtsc_site_sample(Some(RIP), &state, None);
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

        collector.record_rdtsc_site_sample(Some(RIP + 2), &state, None);
        collector.record_rdtsc_site_sample(Some(RIP), &state, None);
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
            capped.record_rdtsc_site_sample(Some(RIP), &state, None);
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
    fn entry_clock_ledger_keeps_only_the_first_two_callback_snapshots() {
        use od_isa::LongModeState;

        const RIP: u64 = 0xffff_ffff_8271_b31b;
        let mut collector =
            LinuxExecutionObservationCollector::default().with_rdtsc_entry_sample_rip(Some(RIP));
        let state = LongModeState::default();
        for hit in 1..=4 {
            assert_eq!(
                collector.wants_rdtsc_clock_trace(),
                hit <= LINUX_RDTSC_CLOCK_TRACE_CAPACITY as u64
            );
            collector.record_rdtsc_site_sample(
                Some(RIP),
                &state,
                (hit <= LINUX_RDTSC_CLOCK_TRACE_CAPACITY as u64).then_some(RdtscClockReadLedger {
                    initial_now_ns: 10,
                    now_ns: 20 + 2 * hit,
                    tsc: 0x1000 + hit,
                    tsc_hz: 2_400_000_000,
                    retired_instructions_charged: 5,
                    pm_timer_ns_charged: 2,
                    pit_ns_charged: 1,
                    rdtsc_ns_charged: 1,
                    idle_ns_advanced: 2 + hit,
                    quantum_ordinal: hit + 9,
                    irqs_delivered: hit - 1,
                }),
            );
            if hit <= LINUX_RDTSC_CLOCK_TRACE_CAPACITY as u64 {
                collector.complete_rdtsc_clock_trace(
                    Some(RIP),
                    RdtscClockReadCompletion {
                        quantum_instructions_executed: 3,
                        retired_instructions_through_rdtsc: 8,
                    },
                );
            }
        }
        assert!(!collector.wants_rdtsc_clock_trace());
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_entry_clock_trace_samples.len(), 2);
        assert_eq!(observations.rdtsc_entry_clock_trace_missing, 0);
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].hit_ordinal,
            1
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].callback_tsc,
            0x1001
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].retired_ns_charged,
            Some(5)
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].quantum_instructions_executed,
            3
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].retired_instructions_through_rdtsc,
            8
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].quantum_ordinal,
            10
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].irqs_delivered,
            0
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[0].unaccounted_elapsed_ns,
            Some(0)
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[1].hit_ordinal,
            2
        );
        assert_eq!(
            observations.rdtsc_entry_clock_trace_samples[1].unaccounted_elapsed_ns,
            Some(1)
        );
    }

    #[test]
    fn entry_clock_ledger_counts_missing_only_for_the_first_two_hits() {
        use od_isa::LongModeState;

        const RIP: u64 = 0xffff_ffff_8271_b31b;
        let mut collector =
            LinuxExecutionObservationCollector::default().with_rdtsc_entry_sample_rip(Some(RIP));
        for _ in 0..3 {
            collector.record_rdtsc_site_sample(Some(RIP), &LongModeState::default(), None);
        }
        assert!(!collector.wants_rdtsc_clock_trace());
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_entry_clock_trace_samples.len(), 0);
        assert_eq!(observations.rdtsc_entry_clock_trace_missing, 2);
    }

    #[test]
    fn inactive_or_complete_clock_trace_does_not_build_diagnostic_state() {
        use od_isa::LongModeState;

        const RIP: u64 = 0xffff_ffff_8271_b31b;
        let mut builds = 0;
        let inactive = LinuxExecutionObservationCollector::default();
        assert_eq!(
            inactive.build_rdtsc_clock_trace_if_active(|| {
                builds += 1;
                ()
            }),
            None
        );
        let mut inactive = inactive;
        inactive.complete_rdtsc_clock_trace(
            Some(RIP),
            RdtscClockReadCompletion {
                quantum_instructions_executed: 2,
                retired_instructions_through_rdtsc: 2,
            },
        );

        let mut complete =
            LinuxExecutionObservationCollector::default().with_rdtsc_entry_sample_rip(Some(RIP));
        for _ in 0..LINUX_RDTSC_CLOCK_TRACE_CAPACITY {
            assert!(complete.wants_rdtsc_clock_trace());
            assert_eq!(
                complete.build_rdtsc_clock_trace_if_active(|| {
                    builds += 1;
                    ()
                }),
                Some(())
            );
            complete.record_rdtsc_site_sample(Some(RIP), &LongModeState::default(), None);
        }
        complete.complete_rdtsc_clock_trace(
            Some(RIP),
            RdtscClockReadCompletion {
                quantum_instructions_executed: 2,
                retired_instructions_through_rdtsc: 2,
            },
        );
        assert!(!complete.wants_rdtsc_clock_trace());
        assert_eq!(
            complete.build_rdtsc_clock_trace_if_active(|| {
                builds += 1;
                ()
            }),
            None
        );
        assert_eq!(builds, LINUX_RDTSC_CLOCK_TRACE_CAPACITY);
    }

    #[test]
    fn invalid_clock_trace_completion_is_counted_missing() {
        use od_isa::LongModeState;

        const RIP: u64 = 0xffff_ffff_8271_b31b;
        let mut collector =
            LinuxExecutionObservationCollector::default().with_rdtsc_entry_sample_rip(Some(RIP));
        collector.record_rdtsc_site_sample(
            Some(RIP),
            &LongModeState::default(),
            Some(RdtscClockReadLedger {
                initial_now_ns: 0,
                now_ns: 0,
                tsc: 0x1000,
                tsc_hz: 2_400_000_000,
                retired_instructions_charged: u64::MAX,
                pm_timer_ns_charged: 0,
                pit_ns_charged: 0,
                rdtsc_ns_charged: 0,
                idle_ns_advanced: 0,
                quantum_ordinal: 1,
                irqs_delivered: 0,
            }),
        );
        collector.complete_rdtsc_clock_trace(
            Some(RIP),
            RdtscClockReadCompletion {
                quantum_instructions_executed: 1,
                retired_instructions_through_rdtsc: u64::MAX,
            },
        );
        let observations = collector.finish();
        assert!(observations.rdtsc_entry_clock_trace_samples.is_empty());
        assert_eq!(observations.rdtsc_entry_clock_trace_missing, 1);
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
            collector.record_rdtsc_site_sample(Some(RIP), &state, None);
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
            collector.record_rdtsc_site_sample(Some(RIP), &state, None);
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
            collector.record_rdtsc_site_sample(Some(ENTRY_RIP), &state, None);
            collector.record_rdtsc_site_sample(Some(POLL_RIP), &state, None);
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
            collector.record_rdtsc_site_sample(Some(RIP), &state, None);
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

    #[test]
    fn rdtsc_prefix_stops_at_sentinel_with_complete_post_report_counts() {
        use od_isa::{LongModeState, Register};
        let sentinel = LINUX_RDTSC_PREFIX_SENTINEL_RIP;
        let mut state = LongModeState::default();
        state.cs_selector = Some(0x08);
        state.cr3 = Some(0x4000);
        state.gpr[Register::Rsp as usize] = 0x5000;
        state.gpr[Register::Rbp as usize] = 0x6000;
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_prefix_sentinel(Some(sentinel));
        for (rip, retired) in [(0xffff_ffff_8100_1000, 4), (sentinel, 7)] {
            collector.record_cpl0_rdtsc(Some(rip));
            let ordinal = collector.record_rdtsc_prefix(
                Some(rip),
                &state,
                Some(RdtscClockReadLedger {
                    initial_now_ns: 10,
                    now_ns: 20,
                    tsc: 123 + retired,
                    tsc_hz: 2_400_000_000,
                    retired_instructions_charged: retired - 1,
                    pm_timer_ns_charged: 0,
                    pit_ns_charged: 0,
                    rdtsc_ns_charged: 1,
                    idle_ns_advanced: 0,
                    quantum_ordinal: retired,
                    irqs_delivered: 0,
                }),
                retired,
                2,
            );
            collector.complete_rdtsc_prefix(
                ordinal,
                RdtscClockReadCompletion {
                    quantum_instructions_executed: 1,
                    retired_instructions_through_rdtsc: retired,
                },
            );
        }
        assert_eq!(
            collector.rdtsc_prefix_status,
            RdtscPrefixStatus::SentinelSeen
        );
        assert!(!collector.wants_rdtsc_prefix());
        let mut built = false;
        assert!(
            collector
                .build_rdtsc_clock_trace_if_active(|| built = true)
                .is_none()
        );
        assert!(!built);
        // Site accounting can continue for the legacy RDTSC table; prefix capture stays frozen.
        collector.record_cpl0_rdtsc(Some(0xffff_ffff_8100_2000));
        assert_eq!(collector.rdtsc_prefix_samples.len(), 2);
        let observations = collector.finish();
        assert_eq!(
            observations.rdtsc_prefix_status,
            RdtscPrefixStatus::SentinelSeen
        );
        assert_eq!(observations.rdtsc_prefix_missing, 0);
        let sample = observations.rdtsc_prefix_samples[1];
        assert_eq!(sample.ordinal, 2);
        assert_eq!(sample.rip, sentinel);
        assert_eq!(sample.cpl, Some(0));
        assert_eq!(sample.cr3, Some(0x4000));
        assert_eq!((sample.rsp, sample.rbp), (0x5000, 0x6000));
        assert_eq!(sample.page_faults_delivered_through, 7);
        assert_eq!(sample.general_protection_faults_delivered_through, 2);
        assert_eq!(sample.clock.retired_instructions_through_rdtsc, 7);
    }

    #[test]
    fn rdtsc_prefix_reports_capacity_and_missing_post_cap_events() {
        use od_isa::LongModeState;
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_prefix_sentinel(Some(LINUX_RDTSC_PREFIX_SENTINEL_RIP));
        collector.rdtsc_prefix_event_ordinal = LINUX_RDTSC_PREFIX_CAPACITY as u64 - 1;
        collector.record_cpl0_rdtsc(Some(0xffff_ffff_8100_1000));
        let ordinal = collector.record_rdtsc_prefix(
            Some(0xffff_ffff_8100_1000),
            &LongModeState::default(),
            Some(RdtscClockReadLedger {
                initial_now_ns: 0,
                now_ns: 1,
                tsc: 1,
                tsc_hz: 1,
                retired_instructions_charged: 0,
                pm_timer_ns_charged: 0,
                pit_ns_charged: 0,
                rdtsc_ns_charged: 0,
                idle_ns_advanced: 0,
                quantum_ordinal: 1,
                irqs_delivered: 0,
            }),
            0,
            0,
        );
        collector.complete_rdtsc_prefix(
            ordinal,
            RdtscClockReadCompletion {
                quantum_instructions_executed: 0,
                retired_instructions_through_rdtsc: 0,
            },
        );
        assert_eq!(
            collector.rdtsc_prefix_status,
            RdtscPrefixStatus::CapacityExhausted
        );
        assert!(!collector.wants_rdtsc_prefix());
        collector.record_cpl0_rdtsc(Some(0xffff_ffff_8100_2000));
        collector.record_cpl0_rdtsc(Some(0xffff_ffff_8100_3000));
        let observations = collector.finish();
        assert_eq!(observations.rdtsc_prefix_missing, 2);
        assert_eq!(observations.rdtsc_prefix_samples.len(), 1);
    }

    #[test]
    fn rdtsc_prefix_marks_missing_callback_data_and_run_end_before_sentinel() {
        use od_isa::LongModeState;
        let mut collector = LinuxExecutionObservationCollector::default()
            .with_rdtsc_prefix_sentinel(Some(LINUX_RDTSC_PREFIX_SENTINEL_RIP));
        collector.record_cpl0_rdtsc(Some(0xffff_ffff_8100_1000));
        collector.record_rdtsc_prefix(
            Some(0xffff_ffff_8100_1000),
            &LongModeState::default(),
            None,
            0,
            0,
        );
        let observations = collector.finish();
        assert_eq!(
            observations.rdtsc_prefix_status,
            RdtscPrefixStatus::RunEndedBeforeSentinel
        );
        assert_eq!(observations.rdtsc_prefix_missing, 1);
        assert!(observations.rdtsc_prefix_samples.is_empty());
    }
}
