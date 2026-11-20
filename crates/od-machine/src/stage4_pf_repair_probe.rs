//! Strict S4-33c CR0.WP write fault -> #PF delivery -> PTE repair -> INVLPG -> LEA -> IRETQ ->
//! retried store -> load probe.
//!
//! The image is a fixed raw profile (`Stage4PfRepairProbeV1`): six page-table pages, one main
//! code page, one handler page, IDT, GDT, two stack pages and one supervisor read-only data
//! page whose PTE is also reachable through a supervisor RW alias of its page table. Execution
//! reuses the accepted S4-19b/c/d/e1/e2 primitives (`step_paged_cr0_wp_mov`,
//! `deliver_paged_page_fault`, `step_paged_cr0_wp_page_table_repair_mov`, `step_paged_invlpg`,
//! `step_paged_exception_stack_discard`, `step_paged_iretq`) unchanged; this module only loads
//! the image, sequences the eight steps under a step budget and records the guest state after
//! each one. The CPU handoff (GDTR, IDTR, CR0/CR3/CR4/EFER, RBX/RCX/RDI/RSI/RSP) is supplied by
//! the loader. The uncached tracked walker is used, so neither TLB behaviour nor the necessity
//! of INVLPG is exercised.

use crate::{
    AccessWidth, AdUpdate, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PagedExceptionDeliveryError, PagedExceptionDeliveryOutcome,
    RamLimits, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_IMAGE_GPA,
    STAGE4_FLAT_PROBE_RAM_BYTES, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    deliver_paged_page_fault, step_paged_cr0_wp_mov, step_paged_cr0_wp_page_table_repair_mov,
    step_paged_exception_stack_discard, step_paged_invlpg, step_paged_iretq,
};
use od_isa::memory::{
    AccessContext, AccessOrigin, InstructionMode, PageFaultRecord, PrivilegeLevel,
};
use od_isa::state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache, Gdtr, Idtr};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register, StiShadow};

pub const STAGE4_PF_REPAIR_IMAGE_BYTES: usize = 0xd000;
pub const STAGE4_PF_REPAIR_MAIN_GVA: u64 = 0x0040_1000;
pub const STAGE4_PF_REPAIR_HANDLER_GVA: u64 = 0x0040_2000;
pub const STAGE4_PF_REPAIR_PTE_ALIAS_GVA: u64 = 0x0040_3000;
pub const STAGE4_PF_REPAIR_IDT_GVA: u64 = 0x0060_0000;
pub const STAGE4_PF_REPAIR_GDT_GVA: u64 = 0x0060_1000;
pub const STAGE4_PF_REPAIR_DATA_GVA: u64 = 0x0080_0000;
pub const STAGE4_PF_REPAIR_INITIAL_RSP: u64 = 0x0070_2027;
/// GPA of the data page and of the page table that holds its PTE.
pub const STAGE4_PF_REPAIR_DATA_GPA: u64 = 0xd000;
pub const STAGE4_PF_REPAIR_PTE_GPA: u64 = 0x6000;
/// PTE values: supervisor read-only (A clear), the repaired value and the end state (A and D).
pub const STAGE4_PF_REPAIR_PTE_RO: u64 = 0xd001;
pub const STAGE4_PF_REPAIR_PTE_RO_ACCESSED: u64 = 0xd021;
pub const STAGE4_PF_REPAIR_PTE_RW: u64 = 0xd023;
pub const STAGE4_PF_REPAIR_PTE_RW_DIRTY: u64 = 0xd063;
/// Raw-image offsets of the initial, canary and store qwords.
pub const STAGE4_PF_REPAIR_INITIAL_OFFSET: usize = 0xc000;
pub const STAGE4_PF_REPAIR_CANARY_OFFSET: usize = 0xc008;
pub const STAGE4_PF_REPAIR_STORE_OFFSET: usize = 0xc010;
/// Sequencer steps of the full case and instructions that retire in it.
pub const STAGE4_PF_REPAIR_STEPS: u64 = 8;
pub const STAGE4_PF_REPAIR_INSTRUCTIONS: u64 = 6;
/// RIP after the final load.
pub const STAGE4_PF_REPAIR_FINAL_RIP: u64 = 0x0040_1006;

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const HANDLER_GPA: u64 = 0x8000;
const MAIN_GPA: u64 = 0x7000;
const IDT_GPA: u64 = 0x9000;
const GDT_GPA: u64 = 0xa000;
const STACK_LOWER_GVA: u64 = 0x0070_1000;
const STACK_LOWER_GPA: u64 = 0xb000;
const STACK_UPPER_GPA: u64 = 0xc000;
const MAIN_CODE: [u8; 6] = [0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03];
const HANDLER_CODE: [u8; 13] = [
    0x48, 0x89, 0x37, // MOV [RDI],RSI
    0x0f, 0x01, 0x3b, // INVLPG [RBX]
    0x48, 0x8d, 0x64, 0x24, 0x08, // LEA RSP,[RSP+8]
    0x48, 0xcf, // IRETQ
];
const HANDLER_NO_LEA: [u8; 8] = [0x48, 0xcf, 0, 0, 0, 0, 0, 0];
const GATE_OFFSET: usize = 14 * 16;
const GATE: [u8; 16] = [
    0x00, 0x20, 0x08, 0x00, 0x00, 0x8e, 0x40, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
];
const GDT: [u8; 24] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0, 0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0,
];
const ENTRIES: [(u64, u64); 13] = [
    (0x1000, 0x2027),
    (0x2000, 0x3027),
    (0x3010, 0x4027), // code table at 4 MiB
    (0x3018, 0x5027), // IDT/GDT/stack table at 6 MiB
    (0x3020, 0x6007), // data table at 8 MiB, A clear
    (0x4008, 0x7027), // main code
    (0x4010, 0x8027), // handler
    (0x4018, 0x6027), // RW alias of the data page table
    (0x5000, 0x9027), // IDT
    (0x5008, 0xa027), // GDT
    (0x5808, 0xb027), // stack, lower page
    (0x5810, 0xc027), // stack
    (0x6000, STAGE4_PF_REPAIR_PTE_RO),
];

/// Deliberate regressions used to prove the oracle sees a broken case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PfRepairInjection {
    None,
    /// The handler stores the read-only PTE value, so the retried store faults again.
    HandlerKeepsPteReadOnly,
    /// The handler has no `LEA RSP,[RSP+8]`: IRETQ pops the error code as RIP.
    HandlerSkipsLea,
    /// The loader maps the data page RW from the start: no #PF is delivered.
    LoaderMapsPageWritable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PfRepairStep {
    Fault,
    Delivery,
    Repair,
    Invlpg,
    Discard,
    Iretq,
    Retry,
    Load,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4PfRepairStop {
    /// All planned steps finished.
    Completed,
    BudgetExhausted,
    /// The first store retired: the page was writable and no #PF was raised.
    NoPageFault,
    /// A step after the initial fault raised a page fault.
    SecondPageFault(Stage4PfRepairStep),
    DeliveryFailed(PagedExceptionDeliveryOutcome),
    /// A step did not retire and raised no page fault.
    Step(Stage4PfRepairStep, &'static str),
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stage4PfRepairFault {
    pub vector: u8,
    pub error_code: Option<u32>,
    pub cr2: u64,
    pub saved_rip: u64,
}

/// One sequencer step and the guest state right after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PfRepairEvent {
    pub step: Stage4PfRepairStep,
    pub retired: bool,
    pub exit: &'static str,
    pub detail: Option<String>,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cr2: Option<u64>,
    pub rax: u64,
    /// Data PTE (GPA 0x6000), data qword and canary qword after the step.
    pub pte: u64,
    pub data_qword: u64,
    pub canary_qword: u64,
    pub page_fault: Option<Stage4PfRepairFault>,
    /// Delivery only: the qword on top of the stack (the #PF error code).
    pub error_code_on_stack: Option<u64>,
    /// IRETQ only: the qword IRETQ pops as RIP, read from the stack before the step.
    pub iretq_frame_rip: Option<u64>,
    pub ad_updates: Vec<AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PfRepairReport {
    pub stop: Stage4PfRepairStop,
    /// Sequencer steps executed, including a step that did not retire.
    pub steps: u64,
    /// Guest instructions retired.
    pub retired: u64,
    pub events: Vec<Stage4PfRepairEvent>,
}

pub struct LoadedStage4PfRepairProbe {
    bus: MachineBus,
    state: LongModeState,
    initial_qword: u64,
    canary_qword: u64,
    store_qword: u64,
    injection: Stage4PfRepairInjection,
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

/// Raw result of one primitive, reduced to what the sequencer records.
struct StepOutcome {
    retired: bool,
    exit: &'static str,
    detail: Option<String>,
    page_fault: Option<PageFaultRecord>,
    ad_updates: Vec<AdUpdate>,
    /// Delivery only.
    delivery: Option<PagedExceptionDeliveryOutcome>,
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
        FirmwareBreakpointDelivery(_) => "firmware_breakpoint_delivery",
        Backend(_) => "backend_error",
        PortIo(_) => "port_io",
        Rejected(_) => "rejected",
    }
}

impl LoadedStage4PfRepairProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }
    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }
    pub const fn initial_qword(&self) -> u64 {
        self.initial_qword
    }
    pub const fn canary_qword(&self) -> u64 {
        self.canary_qword
    }
    pub const fn store_qword(&self) -> u64 {
        self.store_qword
    }

    fn qword(&mut self, gpa: u64) -> u64 {
        self.bus
            .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
            .expect("fixture qword is inside RAM")
    }

    pub fn pte(&mut self) -> u64 {
        self.qword(STAGE4_PF_REPAIR_PTE_GPA)
    }
    pub fn data_qword(&mut self) -> u64 {
        self.qword(STAGE4_PF_REPAIR_DATA_GPA)
    }
    pub fn canary(&mut self) -> u64 {
        self.qword(STAGE4_PF_REPAIR_DATA_GPA + 8)
    }

    /// Apply a deliberate regression. Only fixture registers, PTE and handler bytes change.
    pub fn inject(&mut self, injection: Stage4PfRepairInjection) {
        self.injection = injection;
        match injection {
            Stage4PfRepairInjection::None => {}
            Stage4PfRepairInjection::HandlerKeepsPteReadOnly => {
                self.state.gpr[Register::Rsi as usize] = STAGE4_PF_REPAIR_PTE_RO_ACCESSED;
            }
            Stage4PfRepairInjection::HandlerSkipsLea => {
                self.bus
                    .patch_physical_ram(GuestPhysicalAddress::new(HANDLER_GPA + 6), &HANDLER_NO_LEA)
                    .expect("handler page is RAM");
            }
            Stage4PfRepairInjection::LoaderMapsPageWritable => {
                self.bus
                    .write_physical(
                        GuestPhysicalAddress::new(STAGE4_PF_REPAIR_PTE_GPA),
                        AccessWidth::Qword,
                        STAGE4_PF_REPAIR_DATA_GPA | 0x3,
                        |_| {},
                    )
                    .expect("PTE is RAM");
            }
        }
    }

    fn plan(&self) -> Vec<Stage4PfRepairStep> {
        use Stage4PfRepairStep::*;
        if self.injection == Stage4PfRepairInjection::HandlerSkipsLea {
            vec![Fault, Delivery, Repair, Invlpg, Iretq, Retry, Load]
        } else {
            vec![Fault, Delivery, Repair, Invlpg, Discard, Iretq, Retry, Load]
        }
    }

    /// Qword on the guest stack at `rsp` (the stack is mapped by two fixed pages).
    fn stack_qword(&mut self, rsp: u64) -> Option<u64> {
        let gpa = match rsp.checked_sub(STACK_LOWER_GVA)? {
            offset @ 0..=0xfff => STACK_LOWER_GPA + offset,
            offset @ 0x1000..=0x1fff => STACK_UPPER_GPA + (offset - 0x1000),
            _ => return None,
        };
        Some(self.qword(gpa))
    }

    /// Run up to `budget` sequencer steps. Stops on the first step that does not retire.
    pub fn run(
        &mut self,
        budget: u64,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Stage4PfRepairReport {
        let mut events: Vec<Stage4PfRepairEvent> = Vec::new();
        let mut retired = 0;
        let mut steps = 0;
        let mut fault: Option<PageFaultRecord> = None;
        for step in self.plan() {
            if steps >= budget {
                return Stage4PfRepairReport {
                    stop: Stage4PfRepairStop::BudgetExhausted,
                    steps,
                    retired,
                    events,
                };
            }
            let rsp = self.state.gpr[Register::Rsp as usize];
            let iretq_frame_rip = (step == Stage4PfRepairStep::Iretq)
                .then(|| self.stack_qword(rsp))
                .flatten();
            let mut writes = Vec::new();
            let outcome = match self.execute(step, &mut fault, &mut writes) {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Stage4PfRepairReport {
                        stop: Stage4PfRepairStop::Error(error),
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
            let page_fault = outcome.page_fault.as_ref().map(|r| Stage4PfRepairFault {
                vector: r.fault().vector,
                error_code: r.fault().error_code,
                cr2: r.cr2_value(),
                saved_rip: r.saved_rip(),
            });
            let rsp_after = self.state.gpr[Register::Rsp as usize];
            let error_code_on_stack = (step == Stage4PfRepairStep::Delivery
                && outcome.exit == "delivered")
                .then(|| self.stack_qword(rsp_after))
                .flatten();
            let (outcome_retired, outcome_exit, has_page_fault, delivery) = (
                outcome.retired,
                outcome.exit,
                outcome.page_fault.is_some(),
                outcome.delivery,
            );
            let event = Stage4PfRepairEvent {
                step,
                retired: outcome.retired,
                exit: outcome.exit,
                detail: outcome.detail,
                rip: self.state.rip,
                rsp: rsp_after,
                rflags: self.state.rflags,
                cr2: self.state.cr2,
                rax: self.state.gpr[Register::Rax as usize],
                pte: self.pte(),
                data_qword: self.data_qword(),
                canary_qword: self.canary(),
                page_fault,
                error_code_on_stack,
                iretq_frame_rip,
                ad_updates: outcome.ad_updates,
                committed_writes: writes,
            };
            events.push(event);
            let stop = match step {
                // The first store must fault; any retirement means no #PF was raised.
                Stage4PfRepairStep::Fault if outcome_retired => {
                    retired += 1;
                    Some(Stage4PfRepairStop::NoPageFault)
                }
                Stage4PfRepairStep::Fault if has_page_fault => None,
                Stage4PfRepairStep::Fault => Some(Stage4PfRepairStop::Step(step, outcome_exit)),
                Stage4PfRepairStep::Delivery => match delivery {
                    Some(PagedExceptionDeliveryOutcome::PageFaultDelivered) => None,
                    Some(other) => Some(Stage4PfRepairStop::DeliveryFailed(other)),
                    None => Some(Stage4PfRepairStop::Step(step, outcome_exit)),
                },
                _ if outcome_retired => {
                    retired += 1;
                    None
                }
                _ if has_page_fault => Some(Stage4PfRepairStop::SecondPageFault(step)),
                _ => Some(Stage4PfRepairStop::Step(step, outcome_exit)),
            };
            if let Some(stop) = stop {
                return Stage4PfRepairReport {
                    stop,
                    steps,
                    retired,
                    events,
                };
            }
        }
        Stage4PfRepairReport {
            stop: Stage4PfRepairStop::Completed,
            steps,
            retired,
            events,
        }
    }

    fn execute(
        &mut self,
        step: Stage4PfRepairStep,
        fault: &mut Option<PageFaultRecord>,
        writes: &mut Vec<GuestPhysicalRange>,
    ) -> Result<StepOutcome, String> {
        use Stage4PfRepairStep::*;
        let cr3 = STAGE4_FLAT_PROBE_CR3;
        let efer = STAGE4_FLAT_PROBE_EFER;
        let cb = |range| writes.push(range);
        let err = |e: PagedExceptionDeliveryError| format!("{e:?}");
        Ok(match step {
            Fault | Retry | Load => {
                let r = step_paged_cr0_wp_mov(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    INTEGER_FLAGS,
                    cb,
                )
                .map_err(err)?;
                let retired = r.cpu.instructions_executed == 1;
                if step == Fault {
                    *fault = r.page_fault.clone();
                }
                StepOutcome {
                    retired,
                    exit: if retired {
                        "retired"
                    } else if r.page_fault.is_some() {
                        "page_fault"
                    } else {
                        exit_class(&r.cpu.exit)
                    },
                    detail: (!retired).then(|| format!("{:?}", r.cpu.exit)),
                    page_fault: r.page_fault,
                    ad_updates: r.ad_updates,
                    delivery: None,
                }
            }
            Delivery => {
                let original = fault.clone().ok_or("no initial fault to deliver")?;
                let r = deliver_paged_page_fault(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    &original,
                    cb,
                )
                .map_err(err)?;
                let delivered = r.outcome == PagedExceptionDeliveryOutcome::PageFaultDelivered;
                StepOutcome {
                    retired: false,
                    exit: if delivered {
                        "delivered"
                    } else {
                        "undelivered"
                    },
                    detail: Some(format!("{:?}", r.outcome)),
                    page_fault: r.secondary_page_fault,
                    ad_updates: r.ad_updates,
                    delivery: Some(r.outcome),
                }
            }
            Repair => {
                let r = step_paged_cr0_wp_page_table_repair_mov(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    INTEGER_FLAGS,
                    cb,
                )
                .map_err(err)?;
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
                    detail: (!retired).then(|| format!("{:?}", r.cpu.exit)),
                    page_fault: r.page_fault,
                    ad_updates: r.ad_updates,
                    delivery: None,
                }
            }
            Invlpg => {
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
                    delivery: None,
                }
            }
            Discard => {
                let r = step_paged_exception_stack_discard(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    INTEGER_FLAGS,
                    cb,
                )
                .map_err(err)?;
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
                    detail: (!retired).then(|| format!("{:?}", r.cpu.exit)),
                    page_fault: r.page_fault,
                    ad_updates: r.ad_updates,
                    delivery: None,
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
                .map_err(err)?;
                let retired = r.cpu == od_isa::iretq::IretqResult::Retired;
                StepOutcome {
                    retired,
                    exit: if retired {
                        "retired"
                    } else if r.page_fault.is_some() {
                        "page_fault"
                    } else {
                        match r.cpu {
                            od_isa::iretq::IretqResult::Unsupported => "unsupported",
                            od_isa::iretq::IretqResult::Fault(_) => "fault",
                            _ => "read_aborted",
                        }
                    },
                    detail: (!retired).then(|| format!("{:?}", r.cpu)),
                    page_fault: r.page_fault,
                    ad_updates: r.ad_updates,
                    delivery: None,
                }
            }
        })
    }
}

fn qword_at(image: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(image[offset..offset + 8].try_into().expect("fixed qword"))
}

/// The one image this profile accepts for the given payload qwords.
fn expected_image(initial: u64, canary: u64, store: u64) -> Vec<u8> {
    let mut image = vec![0u8; STAGE4_PF_REPAIR_IMAGE_BYTES];
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
    put(STAGE4_PF_REPAIR_DATA_GPA, &initial.to_le_bytes());
    put(STAGE4_PF_REPAIR_DATA_GPA + 8, &canary.to_le_bytes());
    put(STAGE4_PF_REPAIR_DATA_GPA + 16, &store.to_le_bytes());
    image
}

fn validate(
    image: &[u8],
    initial: u64,
    canary: u64,
    store: u64,
) -> Result<(), Stage4FlatProbeLoadError> {
    if initial == canary || initial == store || canary == store {
        return Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual,
        ));
    }
    let expected = expected_image(initial, canary, store);
    // Page tables are reported per entry; every other page (code and its zero tail, IDT, GDT,
    // stack, data) is reported by its page base through `CodeBytes`.
    let table_end = 0x6000 - STAGE4_FLAT_PROBE_IMAGE_GPA as usize + 0x1000;
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
    for page in (table_end..STAGE4_PF_REPAIR_IMAGE_BYTES).step_by(0x1000) {
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

pub fn load_stage4_pf_repair_probe(
    image: &[u8],
) -> Result<LoadedStage4PfRepairProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_PF_REPAIR_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_PF_REPAIR_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let initial_qword = qword_at(image, STAGE4_PF_REPAIR_INITIAL_OFFSET);
    let canary_qword = qword_at(image, STAGE4_PF_REPAIR_CANARY_OFFSET);
    let store_qword = qword_at(image, STAGE4_PF_REPAIR_STORE_OFFSET);
    validate(image, initial_qword, canary_qword, store_qword)?;
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), STAGE4_FLAT_PROBE_RAM_BYTES)
        .map_err(Stage4FlatProbeLoadError::Memory)?;
    let map = MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
        .map_err(Stage4FlatProbeLoadError::Memory)?;
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_FLAT_PROBE_RAM_BYTES),
        vec![],
        vec![],
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
    let mut state = LongModeState {
        rip: STAGE4_PF_REPAIR_MAIN_GVA,
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
            base: STAGE4_PF_REPAIR_GDT_GVA,
            limit: (GDT.len() - 1) as u16,
        }),
        idtr: Some(Idtr {
            base: STAGE4_PF_REPAIR_IDT_GVA,
            limit: (GATE_OFFSET + GATE.len() - 1) as u16,
        }),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rbx as usize] = STAGE4_PF_REPAIR_DATA_GVA;
    state.gpr[Register::Rcx as usize] = store_qword;
    state.gpr[Register::Rdi as usize] = STAGE4_PF_REPAIR_PTE_ALIAS_GVA;
    state.gpr[Register::Rsi as usize] = STAGE4_PF_REPAIR_PTE_RW;
    state.gpr[Register::Rsp as usize] = STAGE4_PF_REPAIR_INITIAL_RSP;
    Ok(LoadedStage4PfRepairProbe {
        bus,
        state,
        initial_qword,
        canary_qword,
        store_qword,
        injection: Stage4PfRepairInjection::None,
    })
}
