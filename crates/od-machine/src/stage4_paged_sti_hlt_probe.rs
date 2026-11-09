//! Strict S4-27i4a paged `STI; HLT` and paged wake on the Long64/CPL0 profile.
//!
//! The image is the S4-27h layout (`Stage4PagedIrqProbeV1`: page tables, main, handler, IDT, GDT,
//! stack and the Local APIC page, 4 KiB pages, CR0/CR4 = BASE, EFER = 0xD00) with a different main
//! code page: `STI; HLT; HLT` (`FB F4 F4`). The handler is the S4-27h `MOV [RBX],EAX; IRETQ`.
//! The sequencer is the caller: it executes the exact paged `STI` and `HLT` steps, requests the
//! vector in the Local APIC and performs the entry through `poll_paged_apic_entry` (the accepted
//! S4-27h delivery), then the S4-27h EOI store and `IRETQ`.
//!
//! Two scenarios share the image:
//! - `WakeFromHlt` starts with IF=0: `STI` sets IF and an interrupt shadow, `HLT` retires in the
//!   shadow (the shadow ends with it) and halts; the vector wakes the CPU and the frame's RIP is
//!   the instruction after `HLT`.
//! - `BoundaryBeforeHlt` starts with IF=1: `STI` leaves no shadow, the vector enters at the
//!   completed boundary before `HLT` and the frame's RIP is the address of `HLT`.
//!
//! Failure model as in S4-27h: refusals and aborts are terminal, nested exceptions are not
//! modelled (S4-33), the memory type of the APIC page is not modelled.

use crate::paged_apic_entry::{PagedApicEntryResult, poll_paged_apic_entry};
use crate::stage4_paged_irq_probe::{
    GATE_PRESENT_TYPE, IDT_GPA, build_bus, context, expected_image, handoff_state, validate,
};
use crate::{
    AccessWidth, AdUpdate, GuestPhysicalAddress, GuestPhysicalRange, LocalApicHandle, MachineBus,
    STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_EFER, STAGE4_PAGED_IRQ_IMAGE_BYTES,
    STAGE4_PAGED_IRQ_INITIAL_RSP, STAGE4_PAGED_IRQ_STACK_GVA, STAGE4_PAGED_IRQ_VECTOR,
    Stage4FlatProbeLoadError, Stage4PagedIrqFault, Stage4PagedIrqPtes, step_paged_hlt,
    step_paged_iretq, step_paged_mov_dword_store, step_paged_sti,
};
use od_isa::memory::MemoryError;
use od_isa::{
    CpuActivity, INTEGER_FLAGS, InterruptDeliveryPhase, InterruptOutsideProfile, LongModeState,
    Register, StiShadow,
};

/// `STI; HLT; HLT`: the first HLT is where the CPU idles, the second one runs after the handler
/// returned.
pub const STAGE4_PAGED_STI_HLT_MAIN_CODE: [u8; 3] = [0xfb, 0xf4, 0xf4];
pub const STAGE4_PAGED_STI_HLT_IMAGE_BYTES: usize = STAGE4_PAGED_IRQ_IMAGE_BYTES;
/// Address of the first `HLT`; a wake returns to the byte after it, a boundary entry to it.
pub const STAGE4_PAGED_STI_HLT_FIRST_HLT_RIP: u64 = 0x0040_1001;
pub const STAGE4_PAGED_STI_HLT_SECOND_HLT_RIP: u64 = 0x0040_1002;
/// RIP after the last `HLT` of the wake scenario.
pub const STAGE4_PAGED_STI_HLT_WAKE_FINAL_RIP: u64 = 0x0040_1003;
/// Sequencer steps and retired instructions of the full wake scenario.
pub const STAGE4_PAGED_STI_HLT_WAKE_STEPS: u64 = 7;
pub const STAGE4_PAGED_STI_HLT_WAKE_INSTRUCTIONS: u64 = 5;
/// Sequencer steps and retired instructions of the full boundary scenario.
pub const STAGE4_PAGED_STI_HLT_BOUNDARY_STEPS: u64 = 6;
pub const STAGE4_PAGED_STI_HLT_BOUNDARY_INSTRUCTIONS: u64 = 4;

const GPA_PTE: [u64; 6] = [0x4008, 0x4010, 0x5000, 0x5008, 0x5010, 0x5800];
const STACK_GPA: u64 = 0xa000;
const STACK_PTE_GPA: u64 = 0x5800;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PagedStiHltScenario {
    WakeFromHlt,
    BoundaryBeforeHlt,
}

/// Deliberate regressions used to prove the oracle sees a broken case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PagedStiHltInjection {
    None,
    /// The IDT gate has P=0: the entry is refused before any commit and the CPU stays halted.
    IdtGateNotPresent,
    /// The stack PTE is read-only (CR0.WP=1): the first frame store aborts the entry.
    StackReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PagedStiHltStep {
    Sti,
    Hlt,
    /// Poll with nothing requested: the halted CPU keeps waiting.
    IdlePoll,
    /// Request the vector, then wake the halted CPU.
    Wake,
    /// Request the vector, then enter at the completed boundary (CPU running).
    Entry,
    Eoi,
    Iretq,
    /// `HLT` after the handler returned (wake scenario).
    ResumeHlt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4PagedStiHltStop {
    /// All planned steps finished.
    Completed,
    BudgetExhausted,
    /// The entry was refused before any CPU or memory commit (CPU unchanged, vector in IRR).
    DeliveryOutsideProfile(InterruptOutsideProfile),
    /// A table or stack access faulted during the entry; nothing was committed to the CPU and the
    /// vector stays in IRR (model; hardware would raise #PF/#NP, nested delivery is S4-33).
    DeliveryAborted {
        phase: InterruptDeliveryPhase,
        cause: MemoryError,
    },
    /// The APIC selection changed during the entry.
    SelectionChanged,
    /// A step did not do what the plan requires.
    Step(Stage4PagedStiHltStep, &'static str),
    Error(String),
}

/// One sequencer step and the guest state right after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PagedStiHltEvent {
    pub step: Stage4PagedStiHltStep,
    pub retired: bool,
    pub exit: &'static str,
    pub detail: Option<String>,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cs: u16,
    pub cr2: Option<u64>,
    /// The CPU is `Halted` (waiting in HLT) after the step.
    pub halted: bool,
    /// The STI interrupt shadow is active after the step.
    pub shadow_blocking: bool,
    /// Local APIC IRR and ISR bits of the vector after the step.
    pub irr: bool,
    pub isr: bool,
    pub ptes: Stage4PagedIrqPtes,
    pub page_fault: Option<Stage4PagedIrqFault>,
    /// Entry only: the five frame qwords at the new RSP, low to high (RIP, CS, RFLAGS, RSP, SS).
    pub frame: Option<[u64; 5]>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PagedStiHltReport {
    pub stop: Stage4PagedStiHltStop,
    /// Sequencer steps executed, including a step that did not complete.
    pub steps: u64,
    /// Guest instructions retired (an entry is not an instruction).
    pub retired: u64,
    pub events: Vec<Stage4PagedStiHltEvent>,
}

pub struct LoadedStage4PagedStiHltProbe {
    bus: MachineBus,
    state: LongModeState,
    apic: LocalApicHandle,
    scenario: Stage4PagedStiHltScenario,
}

struct StepOutcome {
    retired: bool,
    exit: &'static str,
    detail: Option<String>,
    page_fault: Option<od_isa::memory::PageFaultRecord>,
    ad_updates: Vec<AdUpdate>,
    frame: Option<[u64; 5]>,
    stop: Option<Stage4PagedStiHltStop>,
}

impl StepOutcome {
    fn new(exit: &'static str) -> Self {
        Self {
            retired: false,
            exit,
            detail: None,
            page_fault: None,
            ad_updates: Vec::new(),
            frame: None,
            stop: None,
        }
    }
}

impl LoadedStage4PagedStiHltProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }
    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }
    pub fn apic(&self) -> &LocalApicHandle {
        &self.apic
    }

    fn qword(&mut self, gpa: u64) -> u64 {
        self.bus
            .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
            .expect("fixture qword is inside RAM")
    }

    pub fn ptes(&mut self) -> Stage4PagedIrqPtes {
        let [main, handler, idt, gdt, lapic, stack] = GPA_PTE.map(|gpa| self.qword(gpa));
        Stage4PagedIrqPtes {
            main,
            handler,
            idt,
            gdt,
            lapic,
            stack,
        }
    }

    /// Local APIC (IRR, ISR) bits of the probe vector.
    pub fn vector_state(&self) -> (bool, bool) {
        self.apic.vector_state(STAGE4_PAGED_IRQ_VECTOR)
    }

    /// Apply a deliberate regression. Only one fixture byte or one PTE changes.
    pub fn inject(&mut self, injection: Stage4PagedStiHltInjection) {
        match injection {
            Stage4PagedStiHltInjection::None => {}
            Stage4PagedStiHltInjection::IdtGateNotPresent => {
                // Type 0xE with P=0.
                self.bus
                    .patch_physical_ram(
                        GuestPhysicalAddress::new(IDT_GPA + GATE_PRESENT_TYPE as u64),
                        &[0x0e],
                    )
                    .expect("fixture page is RAM");
            }
            Stage4PagedStiHltInjection::StackReadOnly => {
                self.bus
                    .patch_physical_ram(
                        GuestPhysicalAddress::new(STACK_PTE_GPA),
                        &0xa001u64.to_le_bytes(),
                    )
                    .expect("fixture page is RAM");
            }
        }
    }

    fn plan(&self) -> Vec<Stage4PagedStiHltStep> {
        use Stage4PagedStiHltStep::*;
        match self.scenario {
            Stage4PagedStiHltScenario::WakeFromHlt => {
                vec![Sti, Hlt, IdlePoll, Wake, Eoi, Iretq, ResumeHlt]
            }
            Stage4PagedStiHltScenario::BoundaryBeforeHlt => {
                vec![Sti, Entry, Eoi, Iretq, Hlt, IdlePoll]
            }
        }
    }

    /// The five qwords at `rsp` (inside the single stack page), low to high.
    fn frame_at(&mut self, rsp: u64) -> Option<[u64; 5]> {
        let offset = rsp.checked_sub(STAGE4_PAGED_IRQ_STACK_GVA)?;
        if offset > 0x1000 - 40 {
            return None;
        }
        let mut frame = [0; 5];
        for (index, slot) in frame.iter_mut().enumerate() {
            *slot = self.qword(STACK_GPA + offset + 8 * index as u64);
        }
        Some(frame)
    }

    /// Run up to `budget` sequencer steps. Stops on the first step that fails its plan.
    pub fn run(
        &mut self,
        budget: u64,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Stage4PagedStiHltReport {
        let mut events: Vec<Stage4PagedStiHltEvent> = Vec::new();
        let mut retired = 0;
        let mut steps = 0;
        for step in self.plan() {
            if steps >= budget {
                return self.report(
                    Stage4PagedStiHltStop::BudgetExhausted,
                    steps,
                    retired,
                    events,
                );
            }
            let mut writes = Vec::new();
            let outcome = match self.execute(step, &mut writes) {
                Ok(outcome) => outcome,
                Err(error) => {
                    return self.report(
                        Stage4PagedStiHltStop::Error(error),
                        steps,
                        retired,
                        events,
                    );
                }
            };
            steps += 1;
            for range in &writes {
                committed_write(*range);
            }
            let (irr, isr) = self.vector_state();
            let page_fault = outcome.page_fault.as_ref().map(|r| Stage4PagedIrqFault {
                vector: r.fault().vector,
                error_code: r.fault().error_code,
                linear_address: r.fault().linear_address,
                cr2: r.cr2_value(),
            });
            events.push(Stage4PagedStiHltEvent {
                step,
                retired: outcome.retired,
                exit: outcome.exit,
                detail: outcome.detail,
                rip: self.state.rip,
                rsp: self.state.gpr[Register::Rsp as usize],
                rflags: self.state.rflags,
                cs: self.state.cs_selector.unwrap_or(0),
                cr2: self.state.cr2,
                halted: self.state.activity == Some(CpuActivity::Halted),
                shadow_blocking: self.state.sti_shadow == Some(StiShadow::Blocking),
                irr,
                isr,
                ptes: self.ptes(),
                page_fault,
                frame: outcome.frame,
                ad_updates: outcome.ad_updates,
                committed_writes: writes,
            });
            if outcome.retired {
                retired += 1;
            }
            let stop = outcome.stop.or_else(|| match step {
                // An entry and an idle poll complete without retiring an instruction.
                Stage4PagedStiHltStep::Wake
                | Stage4PagedStiHltStep::Entry
                | Stage4PagedStiHltStep::IdlePoll => None,
                // EOI must have cleared ISR (and left IRR clear) before IRETQ.
                Stage4PagedStiHltStep::Eoi if outcome.retired && (isr || irr) => {
                    Some(Stage4PagedStiHltStop::Step(step, "isr_not_cleared"))
                }
                _ if outcome.retired => None,
                _ => Some(Stage4PagedStiHltStop::Step(step, outcome.exit)),
            });
            if let Some(stop) = stop {
                return self.report(stop, steps, retired, events);
            }
        }
        self.report(Stage4PagedStiHltStop::Completed, steps, retired, events)
    }

    fn report(
        &self,
        stop: Stage4PagedStiHltStop,
        steps: u64,
        retired: u64,
        events: Vec<Stage4PagedStiHltEvent>,
    ) -> Stage4PagedStiHltReport {
        Stage4PagedStiHltReport {
            stop,
            steps,
            retired,
            events,
        }
    }

    fn execute(
        &mut self,
        step: Stage4PagedStiHltStep,
        writes: &mut Vec<GuestPhysicalRange>,
    ) -> Result<StepOutcome, String> {
        use Stage4PagedStiHltStep::*;
        let cr3 = STAGE4_FLAT_PROBE_CR3;
        let efer = STAGE4_FLAT_PROBE_EFER;
        let cb = |range| writes.push(range);
        Ok(match step {
            Sti | Hlt | ResumeHlt => {
                let r = if step == Sti {
                    step_paged_sti(
                        &mut self.state,
                        &mut self.bus,
                        cr3,
                        efer,
                        context(),
                        INTEGER_FLAGS,
                        cb,
                    )
                } else {
                    step_paged_hlt(
                        &mut self.state,
                        &mut self.bus,
                        cr3,
                        efer,
                        context(),
                        INTEGER_FLAGS,
                        cb,
                    )
                }
                .map_err(|e| format!("{e:?}"))?;
                let retired = r.cpu.instructions_executed == 1;
                StepOutcome {
                    retired,
                    exit: match (retired, &r.cpu.exit) {
                        (true, od_isa::PagedProbeExit::Halted) => "halted",
                        (true, _) => "retired",
                        (false, _) if r.page_fault.is_some() => "page_fault",
                        (false, _) => "not_retired",
                    },
                    detail: (!retired).then(|| format!("{:?} {:?}", r.cpu.exit, r.access_error)),
                    page_fault: r.page_fault,
                    ..StepOutcome::new("")
                }
            }
            IdlePoll => self.enter(false, step, cb)?,
            Wake | Entry => self.enter(true, step, cb)?,
            Eoi => {
                let r = step_paged_mov_dword_store(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    INTEGER_FLAGS,
                    cb,
                )
                .map_err(|e| format!("{e:?}"))?;
                let retired = r.cpu.instructions_executed == 1;
                StepOutcome {
                    retired,
                    exit: if retired {
                        "retired"
                    } else if r.page_fault.is_some() {
                        "page_fault"
                    } else {
                        "not_retired"
                    },
                    detail: (!retired).then(|| format!("{:?} {:?}", r.cpu.exit, r.memory_error)),
                    page_fault: r.page_fault,
                    ..StepOutcome::new("")
                }
            }
            Iretq => {
                let r = step_paged_iretq(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    INTEGER_FLAGS,
                    cb,
                )
                .map_err(|e| format!("{e:?}"))?;
                let retired = r.cpu == od_isa::iretq::IretqResult::Retired;
                StepOutcome {
                    retired,
                    exit: if retired {
                        "retired"
                    } else if r.page_fault.is_some() {
                        "page_fault"
                    } else {
                        "not_retired"
                    },
                    detail: (!retired).then(|| format!("{:?}", r.cpu)),
                    page_fault: r.page_fault,
                    ad_updates: r.ad_updates,
                    ..StepOutcome::new("")
                }
            }
        })
    }

    /// Optionally request the vector (the caller's act), then poll the paged APIC entry. With
    /// `must_enter` a poll that does not enter is a plan failure; without it an idle poll is the
    /// expected outcome and an entry is the failure.
    fn enter(
        &mut self,
        must_enter: bool,
        step: Stage4PagedStiHltStep,
        cb: impl FnMut(GuestPhysicalRange),
    ) -> Result<StepOutcome, String> {
        if must_enter {
            self.apic
                .request_fixed_edge(STAGE4_PAGED_IRQ_VECTOR)
                .map_err(|e| format!("{e:?}"))?;
        }
        let report = poll_paged_apic_entry(
            &mut self.state,
            &mut self.bus,
            &self.apic,
            STAGE4_FLAT_PROBE_CR3,
            STAGE4_FLAT_PROBE_EFER,
            context(),
            INTEGER_FLAGS,
            cb,
        )
        .map_err(|e| format!("{e:?}"))?;
        let mut outcome = match report.result {
            PagedApicEntryResult::Idle => {
                let mut o = StepOutcome::new("idle");
                if must_enter {
                    o.stop = Some(Stage4PagedStiHltStop::Step(step, "idle"));
                }
                o
            }
            PagedApicEntryResult::Entered { new_rsp, .. } => {
                let mut o = StepOutcome::new("entered");
                o.frame = self.frame_at(new_rsp);
                if !must_enter {
                    o.stop = Some(Stage4PagedStiHltStop::Step(step, "unexpected_entry"));
                }
                o
            }
            PagedApicEntryResult::OutsideProfile(reason) => {
                let mut o = StepOutcome::new("outside_profile");
                o.stop = Some(Stage4PagedStiHltStop::DeliveryOutsideProfile(reason));
                o
            }
            PagedApicEntryResult::Aborted { phase, cause } => {
                let mut o = StepOutcome::new("delivery_aborted");
                o.stop = Some(Stage4PagedStiHltStop::DeliveryAborted { phase, cause });
                o
            }
            PagedApicEntryResult::SelectionChanged => {
                let mut o = StepOutcome::new("selection_changed");
                o.stop = Some(Stage4PagedStiHltStop::SelectionChanged);
                o
            }
        };
        outcome.page_fault = report.secondary_page_fault;
        outcome.ad_updates = report.ad_updates;
        outcome.detail = report.access_error.map(|e| format!("{e:?}"));
        Ok(outcome)
    }
}

/// The one image this profile accepts: the S4-27h image with `STI; HLT; HLT` as main code.
pub fn expected_stage4_paged_sti_hlt_image() -> Vec<u8> {
    expected_image(&STAGE4_PAGED_STI_HLT_MAIN_CODE)
}

pub fn load_stage4_paged_sti_hlt_probe(
    image: &[u8],
    scenario: Stage4PagedStiHltScenario,
) -> Result<LoadedStage4PagedStiHltProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_PAGED_STI_HLT_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_PAGED_STI_HLT_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    validate(image, &STAGE4_PAGED_STI_HLT_MAIN_CODE)?;
    let (bus, apic) = build_bus(image)?;
    let rflags = match scenario {
        Stage4PagedStiHltScenario::WakeFromHlt => 0x002,
        Stage4PagedStiHltScenario::BoundaryBeforeHlt => 0x202,
    };
    let state = handoff_state(rflags);
    debug_assert_eq!(
        state.gpr[Register::Rsp as usize],
        STAGE4_PAGED_IRQ_INITIAL_RSP
    );
    Ok(LoadedStage4PagedStiHltProbe {
        bus,
        state,
        apic,
        scenario,
    })
}
