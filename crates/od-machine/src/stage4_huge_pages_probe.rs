//! Strict S4-33e huge-page probe (`Stage4HugePagesProbeV1`): a 2 MiB RAM page and a 1 GiB negative.
//!
//! The image is a fixed raw profile (image GPA `0x1000`, 4 MiB, 6 MiB RAM). Code sits on one 4 KiB
//! page. The data page is one 2 MiB leaf (PDE with PS=1, GVA `0x800000`, frame `0x200000`) with
//! three different qwords at its start, middle and last qword, plus a store target that the case
//! overwrites and reads back. The negative route is a separate PML4E whose PDPT entry has PS=1.
//! Execution reuses the accepted primitives unchanged: `step_paged_mov_2m_ram` (S4-10b) for the
//! data loads and the store, `step_paged_mov` (4 KiB walker, S4-10c) for the 1 GiB negative,
//! `step_paged_pio_u8` and the existing `PagedCpl0ProbeV1` quantum for `HLT`. The module loads the
//! image, supplies RBX before each memory step (the accepted allowlist has no instruction that
//! changes RBX), sequences the steps under a step budget and records the guest state after each
//! one. The CPU handoff (CPL0, RFLAGS.AC=0, CR0/CR4, EFER with NXE=1, RAX/RCX/RDX) is supplied by
//! the loader. The uncached tracked walker is used, so TLB behaviour is not exercised.

use crate::{
    AccessWidth, BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PagedProbeExit, PortAddress, PortRange,
    PortRegionBinding, RamLimits, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_EFER,
    STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_UART_BASE, Stage4FlatProbeLayoutError,
    Stage4FlatProbeLoadError, Uart16550Handle, Uart16550PollingTx, run_paged_probe, step_paged_mov,
    step_paged_mov_2m_ram, step_paged_pio_u8,
};
use od_isa::execute::{PioU8Exit, RunExit};
use od_isa::memory::{
    AccessContext, AccessOrigin, InstructionMode, PageFaultRecord, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register, StiShadow};

pub const STAGE4_HUGE_PAGES_IMAGE_BYTES: usize = 0x40_0000;
pub const STAGE4_HUGE_PAGES_RAM_BYTES: u64 = 0x60_0000;
/// Entry RIP (GVA of the code page).
pub const STAGE4_HUGE_PAGES_ENTRY_GVA: u64 = 0x0040_0000;
/// GVA of the 2 MiB data page and of the operands inside it.
pub const STAGE4_HUGE_PAGES_START_GVA: u64 = 0x0080_0000;
pub const STAGE4_HUGE_PAGES_MIDDLE_GVA: u64 = 0x0090_0000;
pub const STAGE4_HUGE_PAGES_END_GVA: u64 = 0x009f_fff8;
pub const STAGE4_HUGE_PAGES_STORE_GVA: u64 = 0x009f_ff00;
/// First byte after the 2 MiB data page; the next 2 MiB leaf maps another frame.
pub const STAGE4_HUGE_PAGES_PAST_PAGE_GVA: u64 = 0x00a0_0000;
/// GVA routed through PML4E[1] and a PDPT entry with PS=1.
pub const STAGE4_HUGE_PAGES_GIB_GVA: u64 = 0x0000_0080_0000_0000;
/// The 2 MiB frame and the physical addresses of its operands.
pub const STAGE4_HUGE_PAGES_FRAME_GPA: u64 = 0x20_0000;
pub const STAGE4_HUGE_PAGES_START_GPA: u64 = 0x20_0000;
pub const STAGE4_HUGE_PAGES_MIDDLE_GPA: u64 = 0x30_0000;
pub const STAGE4_HUGE_PAGES_END_GPA: u64 = 0x3f_fff8;
pub const STAGE4_HUGE_PAGES_STORE_GPA: u64 = 0x3f_ff00;
pub const STAGE4_HUGE_PAGES_SENTINEL_GPA: u64 = 0x40_0000;
/// Raw-image offsets of the three values (the loader reads them from the image).
pub const STAGE4_HUGE_PAGES_START_OFFSET: usize = 0x1f_f000;
pub const STAGE4_HUGE_PAGES_MIDDLE_OFFSET: usize = 0x2f_f000;
pub const STAGE4_HUGE_PAGES_END_OFFSET: usize = 0x3f_eff8;
/// RAX before the first instruction.
pub const STAGE4_HUGE_PAGES_INITIAL_RAX: u64 = 0xdead_beef_cafe_babe;
/// RCX: the value the store writes at offset `0x1fff00`.
pub const STAGE4_HUGE_PAGES_STORE_VALUE: u64 = 0xc0de_c0de_1234_56ab;
/// Contents of the store target and of the first qword of the next 2 MiB frame in the image.
pub const STAGE4_HUGE_PAGES_STORE_PREFILL: u64 = 0x5a5a_5a5a_5a5a_5a5a;
pub const STAGE4_HUGE_PAGES_SENTINEL: u64 = 0xfeed_face_0bad_f00d;
/// Contents of the 4 KiB frame used by the PDE.PS injection.
pub const STAGE4_HUGE_PAGES_DECOY: u64 = 0xa5a5_a5a5_a5a5_a5a5;
/// Sequencer steps and retired instructions of the 2 MiB case and of the 1 GiB negative.
pub const STAGE4_HUGE_PAGES_STEPS: u64 = 7;
pub const STAGE4_HUGE_PAGES_INSTRUCTIONS: u64 = 7;
pub const STAGE4_HUGE_PAGES_GIB_STEPS: u64 = 1;
/// RIP after the last retired instruction of the 2 MiB case (`HLT` at `0x400010`).
pub const STAGE4_HUGE_PAGES_FINAL_RIP: u64 = 0x0040_0011;
/// Instruction start addresses of the 2 MiB case.
pub const STAGE4_HUGE_PAGES_STEP_RIPS: [u64; 7] = [
    0x0040_0000,
    0x0040_0003,
    0x0040_0006,
    0x0040_0009,
    0x0040_000c,
    0x0040_000f,
    0x0040_0010,
];
/// GPAs of the observed paging entries, in order: PML4E[0], PDPT0[0], PD[2], code PTE, PD[4] (the
/// 2 MiB PDE), PD[5], PML4E[1], PDPT1[0] (the PS=1 entry).
pub const STAGE4_HUGE_PAGES_ENTRY_GPAS: [u64; 8] = [
    0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x3028, 0x1008, 0x5000,
];
/// Initial entry values (A and D clear).
pub const STAGE4_HUGE_PAGES_ENTRIES: [u64; 8] = [
    0x2003,
    0x3003,
    0x4003,
    0x7001,
    0x20_0083 | NX,
    0x40_0083 | NX,
    0x5003,
    0x4000_0083,
];
pub const STAGE4_HUGE_PAGES_ENTRY_NAMES: [&str; 8] = [
    "pml4e0",
    "pdpte0",
    "pde_code_table",
    "code_pte",
    "pde_2m",
    "pde_next_2m",
    "pml4e1",
    "pdpte1_ps",
];

const NX: u64 = 1 << 63;
const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const CODE_GPA: u64 = 0x7000;
const DECOY_GPA: u64 = 0x8000;
const SPARE_PT_GPA: u64 = 0x6000;
const PDE_2M_GPA: u64 = 0x3020;
const PDPT1_GPA: u64 = 0x5000;
const CODE: [u8; 17] = [
    0x48, 0x8b, 0x03, // MOV RAX,[RBX]
    0x48, 0x8b, 0x03, // MOV RAX,[RBX]
    0x48, 0x8b, 0x03, // MOV RAX,[RBX]
    0x48, 0x89, 0x0b, // MOV [RBX],RCX
    0x48, 0x8b, 0x03, // MOV RAX,[RBX]
    0xee, // OUT DX,AL
    0xf4, // HLT
];
const ENTRIES: [(u64, u64); 8] = [
    (0x1000, 0x2003),
    (0x1008, 0x5003),
    (0x2000, 0x3003),
    (0x3010, 0x4003),
    (0x3020, 0x20_0083 | NX),
    (0x3028, 0x40_0083 | NX),
    (0x4000, 0x7001),
    (0x5000, 0x4000_0083),
];

/// Which of the two cases the machine runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4HugePagesCase {
    /// Three loads at start, middle and end of the 2 MiB page, a store, a load back, `OUT`, `HLT`.
    TwoMiB,
    /// One load through the PDPT entry with PS=1: a guest #PF with P and RSVD set.
    GibNegative,
}

/// Deliberate regressions used to prove the oracle sees a broken case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4HugePagesInjection {
    None,
    /// `TwoMiB`: the data PDE loses PS and points at a 4 KiB table (mapping only offset 0 to a
    /// decoy frame): the first load returns the decoy, the second load is #PF with PFEC 0x00.
    PdePsCleared,
    /// `GibNegative`: the walk is routed to a PDE with PS=1 through the 4 KiB walker, which ends
    /// in the host error `UnsupportedPageSize` (the outcome class S4-10a had for PDPTE.PS) with
    /// no guest fault.
    PdptePsHostError,
    /// `TwoMiB`: the end load is one byte past the 2 MiB page and reads a different frame.
    OffsetPastPage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4HugePagesStep {
    /// `48 8B 03` at offset 0 of the 2 MiB page.
    ReadStart,
    /// `48 8B 03` at offset `0x100000`.
    ReadMiddle,
    /// `48 8B 03` at offset `0x1ffff8`.
    ReadEnd,
    /// `48 89 0B` at offset `0x1fff00`: sets D on the PDE.
    Store,
    /// `48 8B 03` at offset `0x1fff00`: reads the stored value back.
    ReadBack,
    /// `EE` through `step_paged_pio_u8`.
    Out,
    /// `F4` through the paged quantum.
    Halt,
    /// `48 8B 03` through the PDPT entry with PS=1.
    GibRead,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4HugePagesStop {
    /// All planned steps finished (and, for the 2 MiB case, the CPU halted).
    Completed,
    BudgetExhausted,
    /// A step raised a page fault without retiring.
    PageFault(Stage4HugePagesStep),
    /// A step was rejected by the memory backend or the 4 KiB walker (no architectural fault).
    Backend(Stage4HugePagesStep),
    /// A step did not retire and raised neither a page fault nor a backend error.
    Step(Stage4HugePagesStep, &'static str),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4HugePagesFault {
    pub vector: u8,
    pub error_code: Option<u32>,
    pub cr2: u64,
    pub saved_rip: u64,
    pub access: String,
}

/// One sequencer step and the guest state right after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4HugePagesEvent {
    pub step: Stage4HugePagesStep,
    pub retired: bool,
    pub exit: &'static str,
    pub detail: Option<String>,
    pub rip: u64,
    pub rax: u64,
    /// RBX the sequencer supplied for the step.
    pub rbx: u64,
    pub cpl: u8,
    /// CR2 of the CPU state. Delivery is not part of the case, so it is never committed.
    pub cr2: Option<u64>,
    pub page_fault: Option<Stage4HugePagesFault>,
    pub memory_error: Option<String>,
    /// Observed paging entries after the step, in `STAGE4_HUGE_PAGES_ENTRY_GPAS` order.
    pub entries: [u64; 8],
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4HugePagesReport {
    pub stop: Stage4HugePagesStop,
    /// Sequencer steps executed, including a step that did not retire.
    pub steps: u64,
    /// Guest instructions retired.
    pub retired: u64,
    pub events: Vec<Stage4HugePagesEvent>,
    /// Bytes the guest wrote to the UART.
    pub serial: Vec<u8>,
}

/// An injection that does not belong to the selected case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stage4HugePagesInjectionMismatch {
    pub case: Stage4HugePagesCase,
    pub injection: Stage4HugePagesInjection,
}

pub struct LoadedStage4HugePagesProbe {
    bus: MachineBus,
    state: LongModeState,
    uart: Uart16550Handle,
    values: [u64; 3],
    case: Stage4HugePagesCase,
    injection: Stage4HugePagesInjection,
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
    memory_error: Option<String>,
}

const PLAN_TWO_MIB: [Stage4HugePagesStep; 7] = [
    Stage4HugePagesStep::ReadStart,
    Stage4HugePagesStep::ReadMiddle,
    Stage4HugePagesStep::ReadEnd,
    Stage4HugePagesStep::Store,
    Stage4HugePagesStep::ReadBack,
    Stage4HugePagesStep::Out,
    Stage4HugePagesStep::Halt,
];
const PLAN_GIB: [Stage4HugePagesStep; 1] = [Stage4HugePagesStep::GibRead];

impl LoadedStage4HugePagesProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }
    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }
    /// The qwords at offset 0, `0x100000` and `0x1ffff8` of the 2 MiB page.
    pub const fn values(&self) -> [u64; 3] {
        self.values
    }
    pub const fn efer(&self) -> u64 {
        STAGE4_FLAT_PROBE_EFER
    }
    pub const fn cpl(&self) -> u8 {
        0
    }
    pub fn ac(&self) -> bool {
        self.state.rflags & (1 << 18) != 0
    }
    pub const fn nxe(&self) -> bool {
        STAGE4_FLAT_PROBE_EFER & (1 << 11) != 0
    }
    pub const fn case(&self) -> Stage4HugePagesCase {
        self.case
    }
    pub const fn injection(&self) -> Stage4HugePagesInjection {
        self.injection
    }

    fn qword(&mut self, gpa: u64) -> u64 {
        self.bus
            .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
            .expect("fixture qword is inside RAM")
    }

    /// Observed paging entries, in `STAGE4_HUGE_PAGES_ENTRY_GPAS` order.
    pub fn entries(&mut self) -> [u64; 8] {
        STAGE4_HUGE_PAGES_ENTRY_GPAS.map(|gpa| self.qword(gpa))
    }

    /// The qword at a guest-physical address (for evidence of the store).
    pub fn physical_qword(&mut self, gpa: u64) -> u64 {
        self.qword(gpa)
    }

    fn write_qword(&mut self, gpa: u64, value: u64) {
        self.bus
            .write_physical(
                GuestPhysicalAddress::new(gpa),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .expect("fixture qword is RAM");
    }

    /// Choose the case. Only the plan changes; the image is untouched. Call before `inject`.
    pub fn select_case(&mut self, case: Stage4HugePagesCase) {
        self.case = case;
    }

    /// Apply a deliberate regression. Only paging entries (and, for `OffsetPastPage`, the operand
    /// the sequencer supplies) change. An injection that belongs to the other case is refused.
    pub fn inject(
        &mut self,
        injection: Stage4HugePagesInjection,
    ) -> Result<(), Stage4HugePagesInjectionMismatch> {
        use Stage4HugePagesCase::*;
        use Stage4HugePagesInjection as I;
        let allowed = match injection {
            I::None => true,
            I::PdePsCleared | I::OffsetPastPage => self.case == TwoMiB,
            I::PdptePsHostError => self.case == GibNegative,
        };
        if !allowed {
            return Err(Stage4HugePagesInjectionMismatch {
                case: self.case,
                injection,
            });
        }
        self.injection = injection;
        match injection {
            I::None | I::OffsetPastPage => {}
            I::PdePsCleared => {
                // PDE[4] points at the spare 4 KiB table; its entry 0 maps the decoy frame, the
                // other 511 entries are not present.
                self.write_qword(PDE_2M_GPA, SPARE_PT_GPA | 0x3 | NX);
                self.write_qword(SPARE_PT_GPA, DECOY_GPA | 0x3 | NX);
            }
            I::PdptePsHostError => {
                // PDPT1[0] points at the PD that holds the 2 MiB PDE.
                self.write_qword(PDPT1_GPA, 0x3003);
            }
        }
        Ok(())
    }

    /// The RBX the sequencer supplies for a memory step.
    fn operand(&self, step: Stage4HugePagesStep) -> Option<u64> {
        use Stage4HugePagesStep::*;
        Some(match step {
            ReadStart => STAGE4_HUGE_PAGES_START_GVA,
            ReadMiddle => STAGE4_HUGE_PAGES_MIDDLE_GVA,
            ReadEnd if self.injection == Stage4HugePagesInjection::OffsetPastPage => {
                STAGE4_HUGE_PAGES_PAST_PAGE_GVA
            }
            ReadEnd => STAGE4_HUGE_PAGES_END_GVA,
            Store | ReadBack => STAGE4_HUGE_PAGES_STORE_GVA,
            GibRead if self.injection == Stage4HugePagesInjection::PdptePsHostError => {
                // PML4E[1] -> PDPT1[0] -> PD -> PDE[4] (PS=1): 0x80_0080_0000.
                STAGE4_HUGE_PAGES_GIB_GVA + STAGE4_HUGE_PAGES_START_GVA
            }
            GibRead => STAGE4_HUGE_PAGES_GIB_GVA,
            Out | Halt => return None,
        })
    }

    /// Bytes the guest wrote to the UART. Drains the transmitter.
    fn drain_serial(&self) -> Vec<u8> {
        let mut serial = Vec::new();
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                serial.push(byte);
            }
        }
        serial
    }

    /// Run up to `budget` sequencer steps. Stops on the first step that does not retire.
    pub fn run(
        &mut self,
        budget: u64,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Stage4HugePagesReport {
        let plan: &[Stage4HugePagesStep] = match self.case {
            Stage4HugePagesCase::TwoMiB => &PLAN_TWO_MIB,
            Stage4HugePagesCase::GibNegative => &PLAN_GIB,
        };
        let mut events: Vec<Stage4HugePagesEvent> = Vec::new();
        let mut retired = 0;
        let mut steps = 0;
        let mut stop = Stage4HugePagesStop::Completed;
        for &step in plan {
            if steps >= budget {
                stop = Stage4HugePagesStop::BudgetExhausted;
                break;
            }
            if let Some(rbx) = self.operand(step) {
                self.state.gpr[Register::Rbx as usize] = rbx;
            }
            let mut writes = Vec::new();
            let outcome = match self.execute(step, &mut writes) {
                Ok(outcome) => outcome,
                Err(error) => {
                    stop = Stage4HugePagesStop::Error(error);
                    break;
                }
            };
            steps += 1;
            for range in &writes {
                committed_write(*range);
            }
            let has_page_fault = outcome.page_fault.is_some();
            let has_memory_error = outcome.memory_error.is_some();
            let (outcome_retired, outcome_exit) = (outcome.retired, outcome.exit);
            let page_fault = outcome.page_fault.as_ref().map(|r| Stage4HugePagesFault {
                vector: r.fault().vector,
                error_code: r.fault().error_code,
                cr2: r.cr2_value(),
                saved_rip: r.saved_rip(),
                access: format!("{:?}", r.access()),
            });
            events.push(Stage4HugePagesEvent {
                step,
                retired: outcome.retired,
                exit: outcome.exit,
                detail: outcome.detail,
                rip: self.state.rip,
                rax: self.state.gpr[Register::Rax as usize],
                rbx: self.state.gpr[Register::Rbx as usize],
                cpl: self.cpl(),
                cr2: self.state.cr2,
                page_fault,
                memory_error: outcome.memory_error,
                entries: self.entries(),
                committed_writes: writes,
            });
            if outcome_retired {
                retired += 1;
            } else if has_page_fault {
                stop = Stage4HugePagesStop::PageFault(step);
                break;
            } else if has_memory_error {
                stop = Stage4HugePagesStop::Backend(step);
                break;
            } else {
                stop = Stage4HugePagesStop::Step(step, outcome_exit);
                break;
            }
        }
        Stage4HugePagesReport {
            stop,
            steps,
            retired,
            events,
            serial: self.drain_serial(),
        }
    }

    fn execute(
        &mut self,
        step: Stage4HugePagesStep,
        writes: &mut Vec<GuestPhysicalRange>,
    ) -> Result<StepOutcome, String> {
        use Stage4HugePagesStep::*;
        let cr3 = STAGE4_FLAT_PROBE_CR3;
        let efer = STAGE4_FLAT_PROBE_EFER;
        let defined = INTEGER_FLAGS;
        let cb = |range| writes.push(range);
        let mov = |r: crate::PagedMovStepReport| {
            let retired = r.cpu.instructions_executed == 1;
            StepOutcome {
                retired,
                exit: if retired {
                    "retired"
                } else if r.page_fault.is_some() {
                    "page_fault"
                } else if r.memory_error.is_some() {
                    "backend_rejected"
                } else if matches!(r.cpu.exit, RunExit::Unsupported { .. }) {
                    "unsupported"
                } else {
                    "not_retired"
                },
                detail: (!retired).then(|| format!("{:?}", r.cpu.exit)),
                page_fault: r.page_fault,
                memory_error: r.memory_error.map(|e| format!("{e:?}")),
            }
        };
        let err = |e: crate::PagedMovStepError| format!("{e:?}");
        Ok(match step {
            ReadStart | ReadMiddle | ReadEnd | Store | ReadBack => mov(step_paged_mov_2m_ram(
                &mut self.state,
                &mut self.bus,
                cr3,
                efer,
                context(),
                defined,
                cb,
            )
            .map_err(err)?),
            GibRead => mov(step_paged_mov(
                &mut self.state,
                &mut self.bus,
                cr3,
                efer,
                context(),
                defined,
                cb,
            )
            .map_err(err)?),
            Out => {
                let r = step_paged_pio_u8(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    defined,
                    cb,
                )
                .map_err(|e| format!("{e:?}"))?;
                let retired =
                    r.cpu.instructions_executed == 1 && matches!(r.cpu.exit, PioU8Exit::Retired);
                StepOutcome {
                    retired,
                    exit: if retired {
                        "retired"
                    } else if r.page_fault.is_some() {
                        "page_fault"
                    } else if r.memory_error.is_some() {
                        "backend_rejected"
                    } else {
                        "not_retired"
                    },
                    detail: (!retired).then(|| format!("{:?}", r.cpu.exit)),
                    page_fault: r.page_fault,
                    memory_error: r.memory_error.map(|e| format!("{e:?}")),
                }
            }
            Halt => {
                let r = run_paged_probe(
                    &mut self.state,
                    &mut self.bus,
                    cr3,
                    efer,
                    context(),
                    1,
                    defined,
                    || false,
                    cb,
                );
                let halted = matches!(r.exit, PagedProbeExit::Halted);
                StepOutcome {
                    retired: halted && r.instructions_executed == 1,
                    exit: if halted { "halted" } else { "not_halted" },
                    detail: (!halted).then(|| format!("{:?}", r.exit)),
                    page_fault: r.page_fault,
                    memory_error: r.memory_error.map(|e| format!("{e:?}")),
                }
            }
        })
    }
}

fn qword_at(image: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(image[offset..offset + 8].try_into().expect("fixed qword"))
}

/// The one image this profile accepts for the given values.
fn expected_image(values: [u64; 3]) -> Vec<u8> {
    let mut image = vec![0u8; STAGE4_HUGE_PAGES_IMAGE_BYTES];
    let base = STAGE4_FLAT_PROBE_IMAGE_GPA;
    let mut put = |gpa: u64, bytes: &[u8]| {
        let o = (gpa - base) as usize;
        image[o..o + bytes.len()].copy_from_slice(bytes);
    };
    for (gpa, entry) in ENTRIES {
        put(gpa, &entry.to_le_bytes());
    }
    put(CODE_GPA, &CODE);
    put(DECOY_GPA, &STAGE4_HUGE_PAGES_DECOY.to_le_bytes());
    put(STAGE4_HUGE_PAGES_START_GPA, &values[0].to_le_bytes());
    put(STAGE4_HUGE_PAGES_MIDDLE_GPA, &values[1].to_le_bytes());
    put(
        STAGE4_HUGE_PAGES_STORE_GPA,
        &STAGE4_HUGE_PAGES_STORE_PREFILL.to_le_bytes(),
    );
    put(STAGE4_HUGE_PAGES_END_GPA, &values[2].to_le_bytes());
    put(
        STAGE4_HUGE_PAGES_SENTINEL_GPA,
        &STAGE4_HUGE_PAGES_SENTINEL.to_le_bytes(),
    );
    image
}

fn validate(image: &[u8], values: [u64; 3]) -> Result<(), Stage4FlatProbeLoadError> {
    // The three values must differ from each other and from every other constant, or two steps
    // (or an injection) would be indistinguishable by RAX.
    let reserved = [
        STAGE4_HUGE_PAGES_DECOY,
        STAGE4_HUGE_PAGES_STORE_PREFILL,
        STAGE4_HUGE_PAGES_SENTINEL,
        STAGE4_HUGE_PAGES_INITIAL_RAX,
        STAGE4_HUGE_PAGES_STORE_VALUE,
    ];
    let distinct = values[0] != values[1] && values[0] != values[2] && values[1] != values[2];
    if !distinct
        || values.iter().any(|v| reserved.contains(v))
        || values[2] & 0xff == STAGE4_HUGE_PAGES_STORE_VALUE & 0xff
    {
        return Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual,
        ));
    }
    let expected = expected_image(values);
    // Page tables are reported per entry; every other page (code, data and zero frames) is
    // reported by its page base through `CodeBytes`.
    let table_end = 0x7000 - STAGE4_FLAT_PROBE_IMAGE_GPA as usize;
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
    for page in (table_end..STAGE4_HUGE_PAGES_IMAGE_BYTES).step_by(0x1000) {
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

pub fn load_stage4_huge_pages_probe(
    image: &[u8],
) -> Result<LoadedStage4HugePagesProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_HUGE_PAGES_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_HUGE_PAGES_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let values = [
        qword_at(image, STAGE4_HUGE_PAGES_START_OFFSET),
        qword_at(image, STAGE4_HUGE_PAGES_MIDDLE_OFFSET),
        qword_at(image, STAGE4_HUGE_PAGES_END_OFFSET),
    ];
    validate(image, values)?;
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), STAGE4_HUGE_PAGES_RAM_BYTES)
        .map_err(Stage4FlatProbeLoadError::Memory)?;
    let map = MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
        .map_err(Stage4FlatProbeLoadError::Memory)?;
    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let uart_range = PortRange::new(PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE), 8)
        .map_err(|_| Stage4FlatProbeLoadError::BusBuild(crate::BusBuildError::InvalidPortRange))?;
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_HUGE_PAGES_RAM_BYTES),
        vec![Box::new(uart_device)],
        vec![],
        vec![PortRegionBinding {
            range: uart_range,
            device_index: 0,
        }],
    )
    .map_err(Stage4FlatProbeLoadError::BusBuild)?;
    bus.write_flat_bytes(
        GuestPhysicalAddress::new(STAGE4_FLAT_PROBE_IMAGE_GPA),
        image,
        |_| {},
    )
    .map_err(|error| match error {
        BusAccessError::Memory(error) => Stage4FlatProbeLoadError::Memory(error),
        _ => Stage4FlatProbeLoadError::RamCoverage {
            required_end: STAGE4_FLAT_PROBE_IMAGE_GPA + image.len() as u64,
            ram_bytes: STAGE4_HUGE_PAGES_RAM_BYTES,
        },
    })?;
    for (offset, byte) in [(3, 0x83), (0, 0x01), (1, 0x00), (3, 0x03)] {
        bus.write_port(
            PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE + offset),
            AccessWidth::Byte,
            byte,
        )
        .map_err(Stage4FlatProbeLoadError::UartConfiguration)?;
    }
    // RFLAGS = 2: AC (bit 18) is clear. The control registers and EFER carry PG/WP/PE and NXE.
    let mut state = LongModeState {
        cr3: None,
        rip: STAGE4_HUGE_PAGES_ENTRY_GVA,
        rflags: 2,
        cr0: Some(CR0),
        cr4: Some(CR4),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = STAGE4_HUGE_PAGES_INITIAL_RAX;
    state.gpr[Register::Rbx as usize] = STAGE4_HUGE_PAGES_START_GVA;
    state.gpr[Register::Rcx as usize] = STAGE4_HUGE_PAGES_STORE_VALUE;
    state.gpr[Register::Rdx as usize] = u64::from(STAGE4_FLAT_PROBE_UART_BASE);
    Ok(LoadedStage4HugePagesProbe {
        bus,
        state,
        uart,
        values,
        case: Stage4HugePagesCase::TwoMiB,
        injection: Stage4HugePagesInjection::None,
    })
}
