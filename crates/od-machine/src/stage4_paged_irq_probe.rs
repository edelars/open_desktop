//! Strict S4-27h one-shot external APIC vector on the paged Long64/CPL0 profile.
//!
//! The image is a fixed raw profile (`Stage4PagedIrqProbeV1`): five page-table pages, one main
//! code page, one handler page, IDT, GDT, one stack page and the Local APIC page mapped at a
//! guest linear address (4 KiB pages, CR0/CR4 = BASE, EFER = 0xD00, uncached tracked walker).
//! Main is two `INVLPG [RBX]` (retired, no data access). The sequencer requests the vector in
//! the Local APIC, delivers it through the paged IDT/stack (`deliver_paged_external_interrupt`),
//! acknowledges IRR -> ISR only after the candidate state is complete, runs the handler's
//! `MOV [RBX],EAX` (`89 03`) EOI store through the paged MMIO path, executes the existing paged
//! `IRETQ` and then one more main instruction. The CPU handoff (GDTR, IDTR, CR0/CR3/CR4/EFER,
//! RAX/RBX/RSP, RFLAGS with IF=1) and the Local APIC software enable (SVR) are supplied by the
//! loader.
//!
//! Failure model: delivery refusals and aborts are terminal and nested exceptions are not
//! modelled (a hardware core would deliver #NP/#PF sequentially; that is S4-33). The memory
//! type of the APIC page (PCD/PWT/PAT) is not modelled.

use crate::{
    AccessWidth, AdUpdate, BusDevice, GuestPhysicalAddress, GuestPhysicalRange,
    LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_IMAGE_GPA,
    STAGE4_FLAT_PROBE_RAM_BYTES, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    deliver_paged_external_interrupt, step_paged_invlpg, step_paged_iretq,
    step_paged_mov_dword_store,
};
use od_isa::memory::{
    AccessContext, AccessOrigin, InstructionMode, MemoryError, PageFaultRecord, PrivilegeLevel,
};
use od_isa::state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache, Gdtr, Idtr};
use od_isa::{
    CpuActivity, INTEGER_FLAGS, InterruptDeliveryPhase, InterruptDeliveryResult,
    InterruptOutsideProfile, LongModeState, Register, StiShadow,
};

pub const STAGE4_PAGED_IRQ_IMAGE_BYTES: usize = 0xa000;
pub const STAGE4_PAGED_IRQ_MAIN_GVA: u64 = 0x0040_1000;
pub const STAGE4_PAGED_IRQ_HANDLER_GVA: u64 = 0x0040_2000;
pub const STAGE4_PAGED_IRQ_IDT_GVA: u64 = 0x0060_0000;
pub const STAGE4_PAGED_IRQ_GDT_GVA: u64 = 0x0060_1000;
/// Linear page that maps the Local APIC page; RBX holds its EOI register (+0xB0).
pub const STAGE4_PAGED_IRQ_LAPIC_GVA: u64 = 0x0060_2000;
pub const STAGE4_PAGED_IRQ_EOI_GVA: u64 = 0x0060_20b0;
pub const STAGE4_PAGED_IRQ_STACK_GVA: u64 = 0x0070_0000;
/// Deliberately not 16-byte aligned, so the delivery alignment is observable.
pub const STAGE4_PAGED_IRQ_INITIAL_RSP: u64 = 0x0070_0ff8;
pub const STAGE4_PAGED_IRQ_VECTOR: u8 = 0x41;
/// GPAs of the stack page and of the PTEs the evidence reports.
pub const STAGE4_PAGED_IRQ_STACK_GPA: u64 = 0xa000;
pub const STAGE4_PAGED_IRQ_MAIN_PTE_GPA: u64 = 0x4008;
pub const STAGE4_PAGED_IRQ_HANDLER_PTE_GPA: u64 = 0x4010;
pub const STAGE4_PAGED_IRQ_IDT_PTE_GPA: u64 = 0x5000;
pub const STAGE4_PAGED_IRQ_GDT_PTE_GPA: u64 = 0x5008;
pub const STAGE4_PAGED_IRQ_LAPIC_PTE_GPA: u64 = 0x5010;
pub const STAGE4_PAGED_IRQ_STACK_PTE_GPA: u64 = 0x5800;
/// Sequencer steps of the full case and instructions that retire in it.
pub const STAGE4_PAGED_IRQ_STEPS: u64 = 5;
pub const STAGE4_PAGED_IRQ_INSTRUCTIONS: u64 = 4;
/// RIP the interrupt returns to (after the first main INVLPG) and RIP after the second one.
pub const STAGE4_PAGED_IRQ_RETURN_RIP: u64 = 0x0040_1003;
pub const STAGE4_PAGED_IRQ_FINAL_RIP: u64 = 0x0040_1006;

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const MAIN_GPA: u64 = 0x6000;
const HANDLER_GPA: u64 = 0x7000;
const IDT_GPA: u64 = 0x8000;
const GDT_GPA: u64 = 0x9000;
const MAIN_CODE: [u8; 6] = [0x0f, 0x01, 0x3b, 0x0f, 0x01, 0x3b];
const HANDLER_CODE: [u8; 4] = [
    0x89, 0x03, // MOV [RBX],EAX: EOI
    0x48, 0xcf, // IRETQ
];
const HANDLER_NO_EOI: [u8; 4] = [0x48, 0xcf, 0, 0];
const GATE_OFFSET: usize = STAGE4_PAGED_IRQ_VECTOR as usize * 16;
const GATE_PRESENT_TYPE: usize = GATE_OFFSET + 5;
const GATE: [u8; 16] = [
    0x00, 0x20, 0x08, 0x00, 0x00, 0x8e, 0x40, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
];
const GDT: [u8; 24] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0, 0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0,
];
/// Initial leaf PTEs: present and writable, A and D clear. The stack PTE is the one the
/// read-only injection replaces.
pub const STAGE4_PAGED_IRQ_PTE_MAIN: u64 = 0x6003;
pub const STAGE4_PAGED_IRQ_PTE_HANDLER: u64 = 0x7003;
pub const STAGE4_PAGED_IRQ_PTE_IDT: u64 = 0x8003;
pub const STAGE4_PAGED_IRQ_PTE_GDT: u64 = 0x9003;
pub const STAGE4_PAGED_IRQ_PTE_LAPIC: u64 = 0xfee0_0003;
pub const STAGE4_PAGED_IRQ_PTE_STACK: u64 = 0xa003;
const ENTRIES: [(u64, u64); 10] = [
    (0x1000, 0x2023),
    (0x2000, 0x3023),
    (0x3010, 0x4023), // code table at 4 MiB
    (0x3018, 0x5023), // IDT/GDT/APIC/stack table at 6 MiB
    (STAGE4_PAGED_IRQ_MAIN_PTE_GPA, STAGE4_PAGED_IRQ_PTE_MAIN),
    (
        STAGE4_PAGED_IRQ_HANDLER_PTE_GPA,
        STAGE4_PAGED_IRQ_PTE_HANDLER,
    ),
    (STAGE4_PAGED_IRQ_IDT_PTE_GPA, STAGE4_PAGED_IRQ_PTE_IDT),
    (STAGE4_PAGED_IRQ_GDT_PTE_GPA, STAGE4_PAGED_IRQ_PTE_GDT),
    (STAGE4_PAGED_IRQ_LAPIC_PTE_GPA, STAGE4_PAGED_IRQ_PTE_LAPIC),
    (STAGE4_PAGED_IRQ_STACK_PTE_GPA, STAGE4_PAGED_IRQ_PTE_STACK),
];
const SVR_ENABLE: u64 = 0x1ff;
const SVR_OFFSET: u64 = 0xf0;

/// Deliberate regressions used to prove the oracle sees a broken case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PagedIrqInjection {
    None,
    /// The IDT gate has P=0: delivery is refused before any commit.
    IdtGateNotPresent,
    /// The stack PTE is read-only (CR0.WP=1): the first frame store aborts delivery.
    StackReadOnly,
    /// The handler is a bare IRETQ: ISR stays set and a second request is not eligible.
    HandlerWithoutEoi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PagedIrqStep {
    Main,
    Delivery,
    Eoi,
    Iretq,
    Resume,
    /// `HandlerWithoutEoi` only: the same vector is requested again.
    Redelivery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4PagedIrqStop {
    /// All planned steps finished.
    Completed,
    BudgetExhausted,
    /// The APIC had no eligible vector at the delivery step.
    NotEligible,
    /// The delivery was refused before any CPU or memory commit.
    DeliveryOutsideProfile(InterruptOutsideProfile),
    /// A table or stack access faulted during delivery; nothing was committed to the CPU and the
    /// vector stays in IRR (model; hardware would raise #PF/#NP, nested delivery is S4-33).
    DeliveryAborted {
        phase: InterruptDeliveryPhase,
        cause: MemoryError,
    },
    /// The same vector was requested again and is not eligible because its ISR bit is set.
    SecondDeliveryBlocked,
    /// A step did not retire.
    Step(Stage4PagedIrqStep, &'static str),
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stage4PagedIrqFault {
    pub vector: u8,
    pub error_code: Option<u32>,
    pub linear_address: Option<u64>,
    pub cr2: u64,
}

/// Leaf PTEs of the mapped pages (the A and D bits are the evidence).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stage4PagedIrqPtes {
    pub main: u64,
    pub handler: u64,
    pub idt: u64,
    pub gdt: u64,
    pub lapic: u64,
    pub stack: u64,
}

/// One sequencer step and the guest state right after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PagedIrqEvent {
    pub step: Stage4PagedIrqStep,
    pub retired: bool,
    pub exit: &'static str,
    pub detail: Option<String>,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cs: u16,
    pub cr2: Option<u64>,
    /// Local APIC IRR and ISR bits of the vector after the step.
    pub irr: bool,
    pub isr: bool,
    pub ptes: Stage4PagedIrqPtes,
    pub page_fault: Option<Stage4PagedIrqFault>,
    /// Delivery only: the five frame qwords at the new RSP, low to high (RIP, CS, RFLAGS, RSP, SS).
    pub frame: Option<[u64; 5]>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PagedIrqReport {
    pub stop: Stage4PagedIrqStop,
    /// Sequencer steps executed, including a step that did not retire.
    pub steps: u64,
    /// Guest instructions retired (a delivery is not an instruction).
    pub retired: u64,
    pub events: Vec<Stage4PagedIrqEvent>,
}

pub struct LoadedStage4PagedIrqProbe {
    bus: MachineBus,
    state: LongModeState,
    apic: LocalApicHandle,
    injection: Stage4PagedIrqInjection,
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

/// Raw result of one step, reduced to what the sequencer records.
struct StepOutcome {
    retired: bool,
    exit: &'static str,
    detail: Option<String>,
    page_fault: Option<PageFaultRecord>,
    ad_updates: Vec<AdUpdate>,
    frame: Option<[u64; 5]>,
    stop: Option<Stage4PagedIrqStop>,
}

fn exit_class<E: std::fmt::Debug>(exit: &od_isa::PagedProbeExit<E>) -> &'static str {
    use od_isa::PagedProbeExit::*;
    match exit {
        Retired => "retired",
        Halted => "halted",
        Shutdown => "shutdown",
        TranslationContextChange(_) => "translation_context_change",
        Unsupported { .. } => "unsupported",
        Fault(_) => "fault",
        Backend(_) => "backend_error",
        PortIo(_) => "port_io",
        Rejected(_) => "rejected",
    }
}

impl LoadedStage4PagedIrqProbe {
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
        Stage4PagedIrqPtes {
            main: self.qword(STAGE4_PAGED_IRQ_MAIN_PTE_GPA),
            handler: self.qword(STAGE4_PAGED_IRQ_HANDLER_PTE_GPA),
            idt: self.qword(STAGE4_PAGED_IRQ_IDT_PTE_GPA),
            gdt: self.qword(STAGE4_PAGED_IRQ_GDT_PTE_GPA),
            lapic: self.qword(STAGE4_PAGED_IRQ_LAPIC_PTE_GPA),
            stack: self.qword(STAGE4_PAGED_IRQ_STACK_PTE_GPA),
        }
    }

    /// Local APIC (IRR, ISR) bits of the probe vector.
    pub fn vector_state(&self) -> (bool, bool) {
        self.apic.vector_state(STAGE4_PAGED_IRQ_VECTOR)
    }

    /// Apply a deliberate regression. Only fixture bytes and one PTE change.
    pub fn inject(&mut self, injection: Stage4PagedIrqInjection) {
        self.injection = injection;
        let patch = |bus: &mut MachineBus, gpa: u64, bytes: &[u8]| {
            bus.patch_physical_ram(GuestPhysicalAddress::new(gpa), bytes)
                .expect("fixture page is RAM");
        };
        match injection {
            Stage4PagedIrqInjection::None => {}
            Stage4PagedIrqInjection::IdtGateNotPresent => {
                // Type 0xE with P=0.
                patch(&mut self.bus, IDT_GPA + GATE_PRESENT_TYPE as u64, &[0x0e]);
            }
            Stage4PagedIrqInjection::StackReadOnly => {
                patch(
                    &mut self.bus,
                    STAGE4_PAGED_IRQ_STACK_PTE_GPA,
                    &0xa001u64.to_le_bytes(),
                );
            }
            Stage4PagedIrqInjection::HandlerWithoutEoi => {
                patch(&mut self.bus, HANDLER_GPA, &HANDLER_NO_EOI);
            }
        }
    }

    fn plan(&self) -> Vec<Stage4PagedIrqStep> {
        use Stage4PagedIrqStep::*;
        if self.injection == Stage4PagedIrqInjection::HandlerWithoutEoi {
            vec![Main, Delivery, Iretq, Resume, Redelivery]
        } else {
            vec![Main, Delivery, Eoi, Iretq, Resume]
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
            *slot = self.qword(STAGE4_PAGED_IRQ_STACK_GPA + offset + 8 * index as u64);
        }
        Some(frame)
    }

    /// Run up to `budget` sequencer steps. Stops on the first step that does not retire.
    pub fn run(
        &mut self,
        budget: u64,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Stage4PagedIrqReport {
        let mut events: Vec<Stage4PagedIrqEvent> = Vec::new();
        let mut retired = 0;
        let mut steps = 0;
        for step in self.plan() {
            if steps >= budget {
                return Stage4PagedIrqReport {
                    stop: Stage4PagedIrqStop::BudgetExhausted,
                    steps,
                    retired,
                    events,
                };
            }
            let mut writes = Vec::new();
            let outcome = match self.execute(step, &mut writes) {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Stage4PagedIrqReport {
                        stop: Stage4PagedIrqStop::Error(error),
                        steps,
                        retired,
                        events,
                    };
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
            events.push(Stage4PagedIrqEvent {
                step,
                retired: outcome.retired,
                exit: outcome.exit,
                detail: outcome.detail,
                rip: self.state.rip,
                rsp: self.state.gpr[Register::Rsp as usize],
                rflags: self.state.rflags,
                cs: self.state.cs_selector.unwrap_or(0),
                cr2: self.state.cr2,
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
                // A delivery is complete without retiring an instruction.
                Stage4PagedIrqStep::Delivery => None,
                // EOI must have cleared ISR (and left IRR clear) before IRETQ.
                Stage4PagedIrqStep::Eoi if outcome.retired && (isr || irr) => {
                    Some(Stage4PagedIrqStop::Step(step, "isr_not_cleared"))
                }
                _ if outcome.retired => None,
                _ => Some(Stage4PagedIrqStop::Step(step, outcome.exit)),
            });
            if let Some(stop) = stop {
                return Stage4PagedIrqReport {
                    stop,
                    steps,
                    retired,
                    events,
                };
            }
        }
        Stage4PagedIrqReport {
            stop: Stage4PagedIrqStop::Completed,
            steps,
            retired,
            events,
        }
    }

    fn execute(
        &mut self,
        step: Stage4PagedIrqStep,
        writes: &mut Vec<GuestPhysicalRange>,
    ) -> Result<StepOutcome, String> {
        use Stage4PagedIrqStep::*;
        let cr3 = STAGE4_FLAT_PROBE_CR3;
        let efer = STAGE4_FLAT_PROBE_EFER;
        let cb = |range| writes.push(range);
        Ok(match step {
            Main | Resume => {
                let r = step_paged_invlpg(
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
                    detail: Some(format!("{:?}", r.cpu.exit)),
                    page_fault: r.page_fault,
                    ad_updates: Vec::new(),
                    frame: None,
                    stop: None,
                }
            }
            Delivery => self.deliver(cb)?,
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
                        exit_class(&r.cpu.exit)
                    },
                    detail: (!retired).then(|| format!("{:?} {:?}", r.cpu.exit, r.memory_error)),
                    page_fault: r.page_fault,
                    ad_updates: Vec::new(),
                    frame: None,
                    stop: None,
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
                    frame: None,
                    stop: None,
                }
            }
            Redelivery => {
                self.apic
                    .request_fixed_edge(STAGE4_PAGED_IRQ_VECTOR)
                    .map_err(|e| format!("{e:?}"))?;
                let blocked = self.apic.eligible().is_none();
                StepOutcome {
                    retired: false,
                    exit: if blocked { "blocked" } else { "eligible" },
                    detail: None,
                    page_fault: None,
                    ad_updates: Vec::new(),
                    frame: None,
                    stop: Some(if blocked {
                        Stage4PagedIrqStop::SecondDeliveryBlocked
                    } else {
                        Stage4PagedIrqStop::Error(
                            "vector became eligible again without a guest EOI".into(),
                        )
                    }),
                }
            }
        })
    }

    /// Request the vector, deliver it on a candidate state and publish the candidate only
    /// together with the IRR -> ISR acknowledge.
    fn deliver(&mut self, cb: impl FnMut(GuestPhysicalRange)) -> Result<StepOutcome, String> {
        let vector = STAGE4_PAGED_IRQ_VECTOR;
        self.apic
            .request_fixed_edge(vector)
            .map_err(|e| format!("{e:?}"))?;
        let outcome = |exit, stop| StepOutcome {
            retired: false,
            exit,
            detail: None,
            page_fault: None,
            ad_updates: Vec::new(),
            frame: None,
            stop,
        };
        if self.apic.eligible() != Some(vector) {
            return Ok(outcome(
                "not_eligible",
                Some(Stage4PagedIrqStop::NotEligible),
            ));
        }
        let mut candidate = self.state.clone();
        let report = deliver_paged_external_interrupt(
            &mut candidate,
            &mut self.bus,
            STAGE4_FLAT_PROBE_CR3,
            STAGE4_FLAT_PROBE_EFER,
            context(),
            INTEGER_FLAGS,
            vector,
            cb,
        )
        .map_err(|e| format!("{e:?}"))?;
        let mut result = match report.delivery {
            InterruptDeliveryResult::Delivered { new_rsp, .. } => {
                if !self
                    .apic
                    .acknowledge_vector(vector, || self.state = candidate)
                {
                    return Err("APIC selection changed during delivery".into());
                }
                let mut done = outcome("delivered", None);
                done.frame = self.frame_at(new_rsp);
                done
            }
            InterruptDeliveryResult::OutsideProfile(reason) => outcome(
                "outside_profile",
                Some(Stage4PagedIrqStop::DeliveryOutsideProfile(reason)),
            ),
            InterruptDeliveryResult::DeliveryAborted { phase, cause } => outcome(
                "delivery_aborted",
                Some(Stage4PagedIrqStop::DeliveryAborted { phase, cause }),
            ),
        };
        result.page_fault = report.secondary_page_fault;
        result.ad_updates = report.ad_updates;
        result.detail = report.access_error.map(|e| format!("{e:?}"));
        Ok(result)
    }
}

fn qword_at(image: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(image[offset..offset + 8].try_into().expect("fixed qword"))
}

/// The one image this profile accepts.
fn expected_image() -> Vec<u8> {
    let mut image = vec![0u8; STAGE4_PAGED_IRQ_IMAGE_BYTES];
    let base = STAGE4_FLAT_PROBE_IMAGE_GPA;
    let mut put = |gpa: u64, bytes: &[u8]| {
        let o = (gpa - base) as usize;
        image[o..o + bytes.len()].copy_from_slice(bytes);
    };
    for (gpa, value) in ENTRIES {
        put(gpa, &value.to_le_bytes());
    }
    put(MAIN_GPA, &MAIN_CODE);
    put(HANDLER_GPA, &HANDLER_CODE);
    put(IDT_GPA + GATE_OFFSET as u64, &GATE);
    put(GDT_GPA, &GDT);
    image
}

fn validate(image: &[u8]) -> Result<(), Stage4FlatProbeLoadError> {
    let expected = expected_image();
    // Page tables are reported per entry; every other page is reported by its page base.
    let table_end = 0x6000 - STAGE4_FLAT_PROBE_IMAGE_GPA as usize;
    for o in (0..table_end).step_by(8) {
        if image[o..o + 8] != expected[o..o + 8] {
            return Err(Stage4FlatProbeLoadError::Layout(
                Stage4FlatProbeLayoutError::PageTableEntry {
                    gpa: STAGE4_FLAT_PROBE_IMAGE_GPA + o as u64,
                    expected: qword_at(&expected, o),
                    actual: qword_at(image, o),
                },
            ));
        }
    }
    for page in (table_end..STAGE4_PAGED_IRQ_IMAGE_BYTES).step_by(0x1000) {
        if image[page..page + 0x1000] != expected[page..page + 0x1000] {
            return Err(Stage4FlatProbeLoadError::Layout(
                Stage4FlatProbeLayoutError::CodeBytes {
                    gpa: STAGE4_FLAT_PROBE_IMAGE_GPA + page as u64,
                },
            ));
        }
    }
    Ok(())
}

pub fn load_stage4_paged_irq_probe(
    image: &[u8],
) -> Result<LoadedStage4PagedIrqProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_PAGED_IRQ_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_PAGED_IRQ_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    validate(image)?;
    let region = |base: u64, len: u64, kind| {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), len)
            .map(|range| MemoryRegionSpec::new(range, kind))
            .map_err(Stage4FlatProbeLoadError::Memory)
    };
    let map = MachineMemoryMap::new(
        36,
        vec![
            region(0, STAGE4_FLAT_PROBE_RAM_BYTES, MemoryRegionKind::Ram)?,
            region(
                LOCAL_APIC_MMIO_BASE,
                LOCAL_APIC_MMIO_SIZE,
                MemoryRegionKind::Mmio,
            )?,
        ],
    )
    .map_err(Stage4FlatProbeLoadError::Memory)?;
    let local = LocalApic::new();
    let apic = local.handle();
    let devices: Vec<Box<dyn BusDevice>> = vec![Box::new(local)];
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_FLAT_PROBE_RAM_BYTES),
        devices,
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .map_err(Stage4FlatProbeLoadError::BusBuild)?;
    bus.write_flat_bytes(
        GuestPhysicalAddress::new(STAGE4_FLAT_PROBE_IMAGE_GPA),
        image,
        |_| {},
    )
    .map_err(|_| Stage4FlatProbeLoadError::RamCoverage {
        required_end: STAGE4_FLAT_PROBE_IMAGE_GPA + image.len() as u64,
        ram_bytes: STAGE4_FLAT_PROBE_RAM_BYTES,
    })?;
    // Loader handoff: software-enable the Local APIC (SVR bit 8, spurious vector 0xFF).
    bus.write_physical(
        GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + SVR_OFFSET),
        AccessWidth::Dword,
        SVR_ENABLE,
        |_| {},
    )
    .map_err(Stage4FlatProbeLoadError::UartConfiguration)?;
    let mut state = LongModeState {
        rip: STAGE4_PAGED_IRQ_MAIN_GVA,
        rflags: 0x202,
        cr0: Some(CR0),
        cr3: Some(STAGE4_FLAT_PROBE_CR3),
        cr4: Some(CR4),
        cs_selector: Some(0x08),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0xc,
        }),
        gdtr: Some(Gdtr {
            base: STAGE4_PAGED_IRQ_GDT_GVA,
            limit: (GDT.len() - 1) as u16,
        }),
        idtr: Some(Idtr {
            base: STAGE4_PAGED_IRQ_IDT_GVA,
            limit: (GATE_OFFSET + GATE.len() - 1) as u16,
        }),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = 0;
    state.gpr[Register::Rbx as usize] = STAGE4_PAGED_IRQ_EOI_GVA;
    state.gpr[Register::Rsp as usize] = STAGE4_PAGED_IRQ_INITIAL_RSP;
    Ok(LoadedStage4PagedIrqProbe {
        bus,
        state,
        apic,
        injection: Stage4PagedIrqInjection::None,
    })
}
