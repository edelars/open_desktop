//! Strict S4-33b CPL3 SYSCALL -> kernel store -> SYSRETQ -> user load probe.
//!
//! The image is a fixed raw profile (`Stage4Cpl3SyscallProbeV1`): six page-table
//! pages, one user code page, one kernel code page and one data page that is
//! mapped twice (supervisor RW for the kernel store, user RO for the final user
//! load). Execution reuses the accepted S4-20c bounded profile
//! (`step_paged_syscall_profile`) unchanged; this module only loads the image,
//! sequences the four admitted instructions under an instruction budget and
//! records the per-step guest state.

use crate::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PagedExceptionDeliveryError, PagedSyscallStepReport,
    RamLimits, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_IMAGE_BYTES, STAGE4_FLAT_PROBE_IMAGE_GPA,
    STAGE4_FLAT_PROBE_RAM_BYTES, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    step_paged_syscall_profile,
};
use od_isa::state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache};
use od_isa::syscall::{SYSCALL_FLAGS, SyscallInstruction, SyscallState, SyscallStepExit};
use od_isa::{CpuActivity, LongModeState, Register, StiShadow};

pub const STAGE4_CPL3_SYSCALL_USER_CODE_GVA: u64 = 0x0040_0000;
pub const STAGE4_CPL3_SYSCALL_KERNEL_CODE_GVA: u64 = 0x0060_0000;
pub const STAGE4_CPL3_SYSCALL_KERNEL_DATA_GVA: u64 = 0x0080_0000;
pub const STAGE4_CPL3_SYSCALL_USER_DATA_GVA: u64 = 0x0080_1000;
pub const STAGE4_CPL3_SYSCALL_DATA_GPA: u64 = 0x9000;
pub const STAGE4_CPL3_SYSCALL_INITIAL_RSP: u64 = 0x0070_2027;
pub const STAGE4_CPL3_SYSCALL_INITIAL_RAX: u64 = 0x0123_4567_89ab_cdef;
/// Raw-image offsets of the initial and kernel-store qwords.
pub const STAGE4_CPL3_SYSCALL_INITIAL_OFFSET: usize = 0x8000;
pub const STAGE4_CPL3_SYSCALL_STORE_OFFSET: usize = 0x8008;
/// Number of instructions in the admitted roundtrip.
pub const STAGE4_CPL3_SYSCALL_INSTRUCTIONS: u64 = 4;
/// Where a successful roundtrip leaves RIP (after the user `MOV RAX,[RBX]`).
pub const STAGE4_CPL3_SYSCALL_FINAL_RIP: u64 = 0x0040_0005;

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const EFER: u64 = 0xd01;
const NX: u64 = 1 << 63;
const USER_CODE: [u8; 5] = [0x0f, 0x05, 0x48, 0x8b, 0x03];
const KERNEL_CODE: [u8; 6] = [0x4c, 0x89, 0x17, 0x48, 0x0f, 0x07];
const USER_CODE_GPA: u64 = 0x7000;
const KERNEL_CODE_GPA: u64 = 0x8000;
const ENTRIES: [(u64, u64); 9] = [
    (0x1000, 0x2007),
    (0x2000, 0x3007),
    (0x3010, 0x4007),      // user code at 4 MiB
    (0x3018, 0x6003),      // kernel code at 6 MiB (supervisor)
    (0x3020, 0x5007),      // data at 8 MiB
    (0x4000, 0x7005),      // user read/execute
    (0x5000, 0x9003 | NX), // supervisor RW alias
    (0x5008, 0x9005 | NX), // user RO alias of the same GPA
    (0x6000, 0x8001),      // supervisor read/execute
];

/// Deliberate regressions used to prove the oracle sees a broken kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4Cpl3SyscallInjection {
    None,
    /// The kernel stores its qword to a different offset of the data page.
    KernelStoreMisdirected,
    /// SYSRETQ retires but the guest stays in CPL0 (kernel CS/SS restored).
    SysretStaysCpl0,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4Cpl3SyscallStop {
    /// All four admitted instructions retired.
    Completed,
    BudgetExhausted,
    /// A step did not retire.
    Step(SyscallStepExit),
    Error(PagedExceptionDeliveryError),
}

/// One executed instruction and the guest state right after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4Cpl3SyscallEvent {
    pub instruction: SyscallInstruction,
    pub report: PagedSyscallStepReport,
    pub rip_after: u64,
    pub cs_after: Option<u16>,
    pub cpl_after: Option<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4Cpl3SyscallReport {
    pub stop: Stage4Cpl3SyscallStop,
    pub retired: u64,
    pub events: Vec<Stage4Cpl3SyscallEvent>,
}

pub struct LoadedStage4Cpl3SyscallProbe {
    bus: MachineBus,
    state: LongModeState,
    defined: u64,
    initial_qword: u64,
    store_qword: u64,
    injection: Stage4Cpl3SyscallInjection,
}

impl LoadedStage4Cpl3SyscallProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }
    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }
    pub const fn initial_qword(&self) -> u64 {
        self.initial_qword
    }
    pub const fn store_qword(&self) -> u64 {
        self.store_qword
    }

    /// Current privilege level derived from the committed CS selector.
    pub fn cpl(&self) -> Option<u8> {
        self.state.cs_selector.map(|s| (s & 3) as u8)
    }

    /// Apply a deliberate regression. Only fixture registers are changed.
    pub fn inject(&mut self, injection: Stage4Cpl3SyscallInjection) {
        self.injection = injection;
        if injection == Stage4Cpl3SyscallInjection::KernelStoreMisdirected {
            // Last qword of the same data page: still mapped and writable.
            self.state.gpr[Register::Rdi as usize] = STAGE4_CPL3_SYSCALL_KERNEL_DATA_GVA + 0xff8;
        }
    }

    pub fn data_qword(&mut self) -> Option<u64> {
        self.bus
            .read_physical(
                GuestPhysicalAddress::new(STAGE4_CPL3_SYSCALL_DATA_GPA),
                AccessWidth::Qword,
            )
            .ok()
    }

    /// Run up to `budget` instructions of the admitted roundtrip. Stops on the
    /// first step that does not retire.
    pub fn run(
        &mut self,
        budget: u64,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Stage4Cpl3SyscallReport {
        use SyscallInstruction::*;
        let mut events = Vec::new();
        let mut retired = 0;
        for instruction in [Syscall, KernelStore, Sysret, UserLoad] {
            if retired >= budget {
                return Stage4Cpl3SyscallReport {
                    stop: Stage4Cpl3SyscallStop::BudgetExhausted,
                    retired,
                    events,
                };
            }
            let report = match step_paged_syscall_profile(
                &mut self.state,
                &mut self.bus,
                self.defined,
                instruction,
                &mut committed_write,
            ) {
                Ok(report) => report,
                Err(error) => {
                    return Stage4Cpl3SyscallReport {
                        stop: Stage4Cpl3SyscallStop::Error(error),
                        retired,
                        events,
                    };
                }
            };
            self.defined = report.cpu.rflags_defined;
            let ok = report.cpu.exit == SyscallStepExit::Retired;
            if ok
                && instruction == Sysret
                && self.injection == Stage4Cpl3SyscallInjection::SysretStaysCpl0
            {
                self.state.cs_selector = Some(8);
                self.state.ss_selector = Some(0x10);
                self.state.cs_cache = Some(CodeSegmentDescriptorCache {
                    base: 0,
                    limit: u32::MAX,
                    access: 0x9b,
                    flags: 0xa,
                });
            }
            let exit = report.cpu.exit.clone();
            events.push(Stage4Cpl3SyscallEvent {
                instruction,
                report,
                rip_after: self.state.rip,
                cs_after: self.state.cs_selector,
                cpl_after: self.cpl(),
            });
            if !ok {
                return Stage4Cpl3SyscallReport {
                    stop: Stage4Cpl3SyscallStop::Step(exit),
                    retired,
                    events,
                };
            }
            retired += 1;
        }
        Stage4Cpl3SyscallReport {
            stop: Stage4Cpl3SyscallStop::Completed,
            retired,
            events,
        }
    }
}

fn entry(image: &[u8], gpa: u64) -> u64 {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    u64::from_le_bytes(image[o..o + 8].try_into().expect("qword within image"))
}

fn qword_at(image: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(image[offset..offset + 8].try_into().expect("fixed qword"))
}

fn validate(image: &[u8]) -> Result<(), Stage4FlatProbeLoadError> {
    for table in [0x1000_u64, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000] {
        for index in 0..512_u64 {
            let gpa = table + index * 8;
            let expected = ENTRIES
                .iter()
                .find_map(|&(g, v)| (g == gpa).then_some(v))
                .unwrap_or(0);
            let actual = entry(image, gpa);
            if actual != expected {
                return Err(Stage4FlatProbeLoadError::Layout(
                    Stage4FlatProbeLayoutError::PageTableEntry {
                        gpa,
                        expected,
                        actual,
                    },
                ));
            }
        }
    }
    for (gpa, code) in [
        (USER_CODE_GPA, &USER_CODE[..]),
        (KERNEL_CODE_GPA, &KERNEL_CODE[..]),
    ] {
        let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
        // Exact code followed by a zero page: no hidden extra instructions.
        if &image[o..o + code.len()] != code
            || image[o + code.len()..o + 0x1000].iter().any(|b| *b != 0)
        {
            return Err(Stage4FlatProbeLoadError::Layout(
                Stage4FlatProbeLayoutError::CodeBytes { gpa },
            ));
        }
    }
    if qword_at(image, STAGE4_CPL3_SYSCALL_INITIAL_OFFSET)
        == qword_at(image, STAGE4_CPL3_SYSCALL_STORE_OFFSET)
    {
        return Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual,
        ));
    }
    Ok(())
}

pub fn load_stage4_cpl3_syscall_probe(
    image: &[u8],
) -> Result<LoadedStage4Cpl3SyscallProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_FLAT_PROBE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    validate(image)?;
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
    let initial_qword = qword_at(image, STAGE4_CPL3_SYSCALL_INITIAL_OFFSET);
    let store_qword = qword_at(image, STAGE4_CPL3_SYSCALL_STORE_OFFSET);
    let mut state = LongModeState {
        rip: STAGE4_CPL3_SYSCALL_USER_CODE_GVA,
        rflags: 0x202,
        cr0: Some(CR0),
        cr3: Some(STAGE4_FLAT_PROBE_CR3),
        cr4: Some(CR4),
        cs_selector: Some(0x23),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0xfb,
            flags: 0xa,
        }),
        ss_selector: Some(0x1b),
        ss_cache: Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0xf3,
            flags: 0xc,
        }),
        syscall: Some(SyscallState {
            efer: EFER,
            star: (0x10u64 << 48) | (8u64 << 32),
            lstar: STAGE4_CPL3_SYSCALL_KERNEL_CODE_GVA,
            fmask: 0x200,
            r11_flags_defined: None,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = STAGE4_CPL3_SYSCALL_INITIAL_RAX;
    state.gpr[Register::Rbx as usize] = STAGE4_CPL3_SYSCALL_USER_DATA_GVA;
    state.gpr[Register::Rdi as usize] = STAGE4_CPL3_SYSCALL_KERNEL_DATA_GVA;
    state.gpr[Register::R10 as usize] = store_qword;
    state.gpr[Register::Rsp as usize] = STAGE4_CPL3_SYSCALL_INITIAL_RSP;
    Ok(LoadedStage4Cpl3SyscallProbe {
        bus,
        state,
        defined: SYSCALL_FLAGS,
        initial_qword,
        store_qword,
        injection: Stage4Cpl3SyscallInjection::None,
    })
}
