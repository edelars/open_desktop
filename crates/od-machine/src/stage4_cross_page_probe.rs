//! Strict S4-33d cross-page read and fetch probe (`Stage4CrossPageProbeV1`).
//!
//! The image is a fixed raw profile: five page-table pages, two non-contiguous code frames and
//! two non-contiguous data frames inside the 128 KiB probe map. A qword value is split across
//! the data page boundary: three bytes end data page 0 and five bytes start data page 1.
//! Execution reuses the accepted primitives unchanged: `step_paged_mov_word_read_cross_page_fetch`
//! (S4-11f), `step_paged_mov` (S4-11b), `step_paged_mov_dword_read` (S4-11c),
//! `step_paged_mov_word_read` (S4-11e), `step_paged_pio_u8` and the existing `PagedCpl0ProbeV1`
//! quantum for `HLT`. This module only loads the image, sequences the six steps under a step
//! budget and records the guest state after each one. The CPU handoff (CPL0, RFLAGS.AC=0,
//! CR0/CR4, EFER with NXE=1, RAX/RBX/RDX) is supplied by the loader. The uncached tracked walker
//! is used, so TLB behaviour is not exercised.

use crate::{
    AccessWidth, BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PagedProbeExit, PortAddress, PortRange,
    PortRegionBinding, RamLimits, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_EFER,
    STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_RAM_BYTES, STAGE4_FLAT_PROBE_UART_BASE,
    Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError, Uart16550Handle, Uart16550PollingTx,
    run_paged_probe, step_paged_mov, step_paged_mov_dword_read, step_paged_mov_word_read,
    step_paged_mov_word_read_cross_page_fetch, step_paged_pio_u8,
};
use od_isa::execute::{PioU8Exit, RunExit};
use od_isa::memory::{
    AccessContext, AccessOrigin, InstructionMode, PageFaultRecord, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register, StiShadow};

pub const STAGE4_CROSS_PAGE_IMAGE_BYTES: usize = 0xd000;
/// First and second code page (GVA) and their non-contiguous frames (GPA).
pub const STAGE4_CROSS_PAGE_CODE0_GVA: u64 = 0x0040_0000;
pub const STAGE4_CROSS_PAGE_CODE1_GVA: u64 = 0x0040_1000;
pub const STAGE4_CROSS_PAGE_CODE0_GPA: u64 = 0x7000;
pub const STAGE4_CROSS_PAGE_CODE1_GPA: u64 = 0x9000;
/// First and second data page (GVA) and their non-contiguous frames (GPA).
pub const STAGE4_CROSS_PAGE_DATA0_GVA: u64 = 0x0080_0000;
pub const STAGE4_CROSS_PAGE_DATA1_GVA: u64 = 0x0080_1000;
pub const STAGE4_CROSS_PAGE_DATA0_GPA: u64 = 0xb000;
pub const STAGE4_CROSS_PAGE_DATA1_GPA: u64 = 0xd000;
/// Entry RIP: the crossing `66 8B 03` starts two bytes before the code page boundary.
pub const STAGE4_CROSS_PAGE_ENTRY_GVA: u64 = 0x0040_0ffe;
/// RBX of the case: three operand bytes on data page 0, five on data page 1.
pub const STAGE4_CROSS_PAGE_OPERAND_GVA: u64 = 0x0080_0ffd;
/// RBX of the same-page control: a copy of the value inside data page 0.
pub const STAGE4_CROSS_PAGE_CONTROL_GVA: u64 = 0x0080_0800;
/// Leaf PTE GPAs: code 0, code 1, data 0, data 1.
pub const STAGE4_CROSS_PAGE_PTE_GPAS: [u64; 4] = [0x4000, 0x4008, 0x5000, 0x5008];
/// Initial PTE values (supervisor, A and D clear). Data pages carry NX.
pub const STAGE4_CROSS_PAGE_PTES: [u64; 4] = [0x7001, 0x9001, 0xb003 | NX, 0xd003 | NX];
/// Raw-image offsets of the value bytes on data page 0 (3 bytes) and data page 1 (5 bytes).
pub const STAGE4_CROSS_PAGE_FIRST_OFFSET: usize = 0xaffd;
pub const STAGE4_CROSS_PAGE_SECOND_OFFSET: usize = 0xc000;
/// RAX before the first instruction; the upper 48 bits are preserved by the word reads.
pub const STAGE4_CROSS_PAGE_INITIAL_RAX: u64 = 0xdead_beef_cafe_babe;
/// Sequencer steps of the case and instructions that retire in it.
pub const STAGE4_CROSS_PAGE_STEPS: u64 = 6;
pub const STAGE4_CROSS_PAGE_INSTRUCTIONS: u64 = 6;
/// RIP after the last retired instruction (the `HLT` at 0x40100a retires and stays after it).
pub const STAGE4_CROSS_PAGE_FINAL_RIP: u64 = 0x0040_100b;
/// Fixed instruction addresses.
pub const STAGE4_CROSS_PAGE_STEP_RIPS: [u64; 6] = [
    0x0040_0ffe,
    0x0040_1001,
    0x0040_1004,
    0x0040_1006,
    0x0040_1009,
    0x0040_100a,
];

const NX: u64 = 1 << 63;
const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const HOLE_GPA: u64 = 0x3_0000;
const CODE_HEAD: [u8; 2] = [0x66, 0x8b];
const CODE_TAIL: [u8; 11] = [
    0x03, // ModRM of the crossing MOV AX,[RBX]
    0x48, 0x8b, 0x03, // MOV RAX,[RBX]
    0x8b, 0x03, // MOV EAX,[RBX]
    0x66, 0x8b, 0x03, // MOV AX,[RBX]
    0xee, // OUT DX,AL
    0xf4, // HLT
];
const DECOY0: [u8; 8] = [0xa5; 8];
const DECOY1: [u8; 8] = [0x5a; 8];
const ENTRIES: [(u64, u64); 8] = [
    (0x1000, 0x2003),
    (0x2000, 0x3003),
    (0x3010, 0x4003), // code table at 4 MiB
    (0x3020, 0x5003), // data table at 8 MiB
    (0x4000, 0x7001),
    (0x4008, 0x9001),
    (0x5000, 0xb003 | NX),
    (0x5008, 0xd003 | NX),
];

/// Deliberate regressions used to prove the oracle sees a broken case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4CrossPageInjection {
    None,
    /// The second data PTE is not present: #PF on the second page, PFEC 0x00.
    SecondDataPteNotPresent,
    /// The second code PTE carries NX: #PF on the fetch of the second code byte, PFEC 0x11.
    SecondCodePteNx,
    /// The loader swaps the frames of the two data pages: wrong bytes, no fault.
    DataFramesSwapped,
    /// The second data page maps outside RAM: backend rejection, no #PF.
    SecondDataPageHole,
}

/// Where the read operand lives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4CrossPageOperand {
    /// RBX = 0x800ffd: the qword and dword reads cross the data page boundary.
    CrossPage,
    /// RBX = 0x800800: a copy of the value inside data page 0, nothing crosses a data page.
    SamePageControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4CrossPageStep {
    /// `66 8B 03` fetched across the code page boundary (S4-11f), word operand on data page 0.
    WordFetchCross,
    /// `48 8B 03` (S4-11b).
    QwordRead,
    /// `8B 03` (S4-11c).
    DwordRead,
    /// `66 8B 03` inside one code page (S4-11e).
    WordRead,
    /// `EE` through `step_paged_pio_u8`.
    Out,
    /// `F4` through the paged quantum.
    Halt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4CrossPageStop {
    /// All planned steps finished and the CPU halted.
    Completed,
    BudgetExhausted,
    /// A step raised a page fault without retiring.
    PageFault(Stage4CrossPageStep),
    /// A step was rejected by the tracked RAM backend (no architectural fault).
    Backend(Stage4CrossPageStep),
    /// A step did not retire and raised neither a page fault nor a backend error.
    Step(Stage4CrossPageStep, &'static str),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4CrossPageFault {
    pub vector: u8,
    pub error_code: Option<u32>,
    pub cr2: u64,
    pub saved_rip: u64,
    pub access: String,
}

/// One sequencer step and the guest state right after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4CrossPageEvent {
    pub step: Stage4CrossPageStep,
    pub retired: bool,
    pub exit: &'static str,
    pub detail: Option<String>,
    pub rip: u64,
    pub rax: u64,
    pub cpl: u8,
    /// CR2 of the CPU state. Delivery is not part of the case, so it is never committed.
    pub cr2: Option<u64>,
    pub page_fault: Option<Stage4CrossPageFault>,
    pub memory_error: Option<String>,
    /// Leaf PTEs after the step: code 0, code 1, data 0, data 1.
    pub ptes: [u64; 4],
    pub committed_writes: Vec<GuestPhysicalRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4CrossPageReport {
    pub stop: Stage4CrossPageStop,
    /// Sequencer steps executed, including a step that did not retire.
    pub steps: u64,
    /// Guest instructions retired.
    pub retired: u64,
    pub events: Vec<Stage4CrossPageEvent>,
    /// Bytes the guest wrote to the UART.
    pub serial: Vec<u8>,
}

pub struct LoadedStage4CrossPageProbe {
    bus: MachineBus,
    state: LongModeState,
    uart: Uart16550Handle,
    value: u64,
    injection: Stage4CrossPageInjection,
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

const PLAN: [Stage4CrossPageStep; 6] = [
    Stage4CrossPageStep::WordFetchCross,
    Stage4CrossPageStep::QwordRead,
    Stage4CrossPageStep::DwordRead,
    Stage4CrossPageStep::WordRead,
    Stage4CrossPageStep::Out,
    Stage4CrossPageStep::Halt,
];

impl LoadedStage4CrossPageProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }
    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }
    /// The little-endian qword whose bytes are split across the data page boundary.
    pub const fn value(&self) -> u64 {
        self.value
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
    pub const fn injection(&self) -> Stage4CrossPageInjection {
        self.injection
    }

    fn qword(&mut self, gpa: u64) -> u64 {
        self.bus
            .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
            .expect("fixture qword is inside RAM")
    }

    /// Leaf PTEs: code 0, code 1, data 0, data 1.
    pub fn ptes(&mut self) -> [u64; 4] {
        STAGE4_CROSS_PAGE_PTE_GPAS.map(|gpa| self.qword(gpa))
    }

    fn write_pte(&mut self, gpa: u64, value: u64) {
        self.bus
            .write_physical(
                GuestPhysicalAddress::new(gpa),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .expect("PTE is RAM");
    }

    /// Choose where RBX points. Only the register changes; the image is untouched.
    pub fn select_operand(&mut self, operand: Stage4CrossPageOperand) {
        self.state.gpr[Register::Rbx as usize] = match operand {
            Stage4CrossPageOperand::CrossPage => STAGE4_CROSS_PAGE_OPERAND_GVA,
            Stage4CrossPageOperand::SamePageControl => STAGE4_CROSS_PAGE_CONTROL_GVA,
        };
    }

    /// Apply a deliberate regression. Only leaf PTEs change.
    pub fn inject(&mut self, injection: Stage4CrossPageInjection) {
        self.injection = injection;
        match injection {
            Stage4CrossPageInjection::None => {}
            Stage4CrossPageInjection::SecondDataPteNotPresent => {
                self.write_pte(STAGE4_CROSS_PAGE_PTE_GPAS[3], 0);
            }
            Stage4CrossPageInjection::SecondCodePteNx => {
                self.write_pte(
                    STAGE4_CROSS_PAGE_PTE_GPAS[1],
                    STAGE4_CROSS_PAGE_PTES[1] | NX,
                );
            }
            Stage4CrossPageInjection::DataFramesSwapped => {
                self.write_pte(STAGE4_CROSS_PAGE_PTE_GPAS[2], STAGE4_CROSS_PAGE_PTES[3]);
                self.write_pte(STAGE4_CROSS_PAGE_PTE_GPAS[3], STAGE4_CROSS_PAGE_PTES[2]);
            }
            Stage4CrossPageInjection::SecondDataPageHole => {
                self.write_pte(STAGE4_CROSS_PAGE_PTE_GPAS[3], HOLE_GPA | 0x3 | NX);
            }
        }
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
    ) -> Stage4CrossPageReport {
        let mut events: Vec<Stage4CrossPageEvent> = Vec::new();
        let mut retired = 0;
        let mut steps = 0;
        let mut stop = Stage4CrossPageStop::Completed;
        for step in PLAN {
            if steps >= budget {
                stop = Stage4CrossPageStop::BudgetExhausted;
                break;
            }
            let mut writes = Vec::new();
            let outcome = match self.execute(step, &mut writes) {
                Ok(outcome) => outcome,
                Err(error) => {
                    stop = Stage4CrossPageStop::Error(error);
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
            let page_fault = outcome.page_fault.as_ref().map(|r| Stage4CrossPageFault {
                vector: r.fault().vector,
                error_code: r.fault().error_code,
                cr2: r.cr2_value(),
                saved_rip: r.saved_rip(),
                access: format!("{:?}", r.access()),
            });
            events.push(Stage4CrossPageEvent {
                step,
                retired: outcome.retired,
                exit: outcome.exit,
                detail: outcome.detail,
                rip: self.state.rip,
                rax: self.state.gpr[Register::Rax as usize],
                cpl: self.cpl(),
                cr2: self.state.cr2,
                page_fault,
                memory_error: outcome.memory_error,
                ptes: self.ptes(),
                committed_writes: writes,
            });
            if outcome_retired {
                retired += 1;
            } else if has_page_fault {
                stop = Stage4CrossPageStop::PageFault(step);
                break;
            } else if has_memory_error {
                stop = Stage4CrossPageStop::Backend(step);
                break;
            } else {
                stop = Stage4CrossPageStop::Step(step, outcome_exit);
                break;
            }
        }
        Stage4CrossPageReport {
            stop,
            steps,
            retired,
            events,
            serial: self.drain_serial(),
        }
    }

    fn execute(
        &mut self,
        step: Stage4CrossPageStep,
        writes: &mut Vec<GuestPhysicalRange>,
    ) -> Result<StepOutcome, String> {
        use Stage4CrossPageStep::*;
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
            WordFetchCross => mov(step_paged_mov_word_read_cross_page_fetch(
                &mut self.state,
                &mut self.bus,
                cr3,
                efer,
                context(),
                defined,
                cb,
            )
            .map_err(err)?),
            QwordRead => mov(step_paged_mov(
                &mut self.state,
                &mut self.bus,
                cr3,
                efer,
                context(),
                defined,
                cb,
            )
            .map_err(err)?),
            DwordRead => mov(step_paged_mov_dword_read(
                &mut self.state,
                &mut self.bus,
                cr3,
                efer,
                context(),
                defined,
                cb,
            )
            .map_err(err)?),
            WordRead => mov(step_paged_mov_word_read(
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

/// The one image this profile accepts for the given value.
fn expected_image(value: u64) -> Vec<u8> {
    let mut image = vec![0u8; STAGE4_CROSS_PAGE_IMAGE_BYTES];
    let base = STAGE4_FLAT_PROBE_IMAGE_GPA;
    let mut put = |gpa: u64, bytes: &[u8]| {
        let o = (gpa - base) as usize;
        image[o..o + bytes.len()].copy_from_slice(bytes);
    };
    for (gpa, entry) in ENTRIES {
        put(gpa, &entry.to_le_bytes());
    }
    put(0x7ffe, &CODE_HEAD);
    put(0x9000, &CODE_TAIL);
    let raw = value.to_le_bytes();
    put(STAGE4_CROSS_PAGE_DATA0_GPA, &DECOY0);
    put(STAGE4_CROSS_PAGE_DATA0_GPA + 0x800, &raw);
    put(STAGE4_CROSS_PAGE_DATA0_GPA + 0xffd, &raw[..3]);
    put(STAGE4_CROSS_PAGE_DATA1_GPA, &raw[3..]);
    put(STAGE4_CROSS_PAGE_DATA1_GPA + 0xff8, &DECOY1);
    image
}

fn validate(image: &[u8], value: u64) -> Result<(), Stage4FlatProbeLoadError> {
    // The qword and the dword read must differ, or the two steps would be indistinguishable.
    if value >> 32 == 0 {
        return Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual,
        ));
    }
    let expected = expected_image(value);
    // Page tables are reported per entry; every other page (code, data and unmapped frames) is
    // reported by its page base through `CodeBytes`.
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
    for page in (table_end..STAGE4_CROSS_PAGE_IMAGE_BYTES).step_by(0x1000) {
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

pub fn load_stage4_cross_page_probe(
    image: &[u8],
) -> Result<LoadedStage4CrossPageProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_CROSS_PAGE_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_CROSS_PAGE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let mut value_bytes = [0u8; 8];
    value_bytes[..3].copy_from_slice(
        &image[STAGE4_CROSS_PAGE_FIRST_OFFSET..STAGE4_CROSS_PAGE_FIRST_OFFSET + 3],
    );
    value_bytes[3..].copy_from_slice(
        &image[STAGE4_CROSS_PAGE_SECOND_OFFSET..STAGE4_CROSS_PAGE_SECOND_OFFSET + 5],
    );
    let value = u64::from_le_bytes(value_bytes);
    validate(image, value)?;
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), STAGE4_FLAT_PROBE_RAM_BYTES)
        .map_err(Stage4FlatProbeLoadError::Memory)?;
    let map = MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
        .map_err(Stage4FlatProbeLoadError::Memory)?;
    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let uart_range = PortRange::new(PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE), 8)
        .map_err(|_| Stage4FlatProbeLoadError::BusBuild(crate::BusBuildError::InvalidPortRange))?;
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_FLAT_PROBE_RAM_BYTES),
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
            ram_bytes: STAGE4_FLAT_PROBE_RAM_BYTES,
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
        rip: STAGE4_CROSS_PAGE_ENTRY_GVA,
        rflags: 2,
        cr0: Some(CR0),
        cr4: Some(CR4),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = STAGE4_CROSS_PAGE_INITIAL_RAX;
    state.gpr[Register::Rbx as usize] = STAGE4_CROSS_PAGE_OPERAND_GVA;
    state.gpr[Register::Rdx as usize] = u64::from(STAGE4_FLAT_PROBE_UART_BASE);
    Ok(LoadedStage4CrossPageProbe {
        bus,
        state,
        uart,
        value,
        injection: Stage4CrossPageInjection::None,
    })
}
