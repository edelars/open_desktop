//! PERF-25: the batched entry of the CPL0 profile set
//! (`run_with_cpl0_profile_set_and_pio_batched`). Every call must end exactly after the instruction
//! at whose end the caller (or the admission of the set) has something to do, and a run made of
//! batched calls must leave what the same run of one-instruction calls leaves.

use std::cell::Cell;

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, MovCr3Long64V1, RunConfig, RunExit,
    TranslationContextChange, run_with_cpl0_profile_set_and_pio,
    run_with_cpl0_profile_set_and_pio_batched,
};
use od_isa::msr::MsrProfile;
use od_isa::state::Gdtr;
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CliProfile,
    CodeSegmentDescriptorCache, CpuActivity, DataSegmentDescriptorCache, GuestMemory, HaltProfile,
    InstructionMode, IretqUserEntry, LongModeState, MemoryError, OperandSize, PortIo,
    PrivilegeLevel, Register, StiProfile, StiShadow,
};

const RIP: u64 = 0x1000;
const IF: u64 = 1 << 9;

/// The data area: GDT, stack and the REP buffers live in it (see the layout below).
const DATA: u64 = 0x8000;
const DATA_SIZE: u64 = 0x6000;
const GDT: u64 = DATA;
const STACK: u64 = DATA + 0x800;
/// The destination (and the `STOS` buffer) and the source of the REP cases.
const REP_DST: u64 = DATA + 0x1000;
const REP_SRC: u64 = DATA + 0x4000;
/// An access to this page answers a page fault (outside of the data area).
const FAULT_PAGE: u64 = 0xf000;
const USER_RIP: u64 = 0x0000_7fff_0000_1000;

/// Code at `RIP`, a data area at `DATA` (filled with a pattern, so that a store is visible) and an
/// optional page that answers a page fault; every other data access is unmapped.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Code {
    code: Vec<u8>,
    data: Vec<u8>,
    fault_page: Option<u64>,
}

impl Code {
    fn new(code: Vec<u8>) -> Self {
        Self {
            code,
            data: (0..DATA_SIZE)
                .map(|index| (index as u8).wrapping_mul(7).wrapping_add(1))
                .collect(),
            fault_page: None,
        }
    }

    fn put(&mut self, address: u64, value: u64) {
        let offset = (address - DATA) as usize;
        self.data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// The error of an access of `length` bytes at `address`, if it does not hit the data area.
    fn refuse(&self, address: u64, length: usize, access: AccessKind) -> Option<MemoryError> {
        let last = address.wrapping_add(length as u64 - 1);
        if let Some(page) = self.fault_page
            && (address & !0xfff == page || last & !0xfff == page)
        {
            return Some(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(address),
            }));
        }
        let inside = address >= DATA && last >= address && last < DATA + DATA_SIZE;
        (!inside).then_some(MemoryError::Unmapped { address, access })
    }
}

impl GuestMemory for Code {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        if let Some(error) = self.refuse(address, destination.len(), AccessKind::Read) {
            return Err(error);
        }
        let offset = (address - DATA) as usize;
        destination.copy_from_slice(&self.data[offset..offset + destination.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        if let Some(error) = self.refuse(address, source.len(), AccessKind::Write) {
            return Err(error);
        }
        let offset = (address - DATA) as usize;
        self.data[offset..offset + source.len()].copy_from_slice(source);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

/// A port that records its writes and fails a write to `fail`.
#[derive(Default)]
struct Port {
    writes: Vec<(u16, u8)>,
    fail: Option<u16>,
}

impl PortIo for Port {
    type Error = &'static str;

    fn read_u8(&mut self, _: u16) -> Result<u8, Self::Error> {
        Ok(0)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        if self.fail == Some(port) {
            return Err("write failed");
        }
        self.writes.push((port, value));
        Ok(())
    }
}

fn config(budget: u64) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: None,
        context: AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
    }
}

fn state(rflags: u64) -> LongModeState {
    LongModeState {
        rip: RIP,
        rflags: 2 | rflags,
        cr0: Some(0x8001_0033),
        cr4: Some(0x20),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

static CLI: CliProfile = CliProfile::NonPviLong64V1;
static STI: StiProfile = StiProfile::NonPviLong64V1;
static HALT: HaltProfile = HaltProfile::Long64V1;

fn base() -> Cpl0ProfileSet<'static> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
}

/// One batched call with `budget`: the result, and the RIP of every boundary the check was asked
/// at (`refuse_at`: the number of the check that says no, counting from 1).
fn batched(
    state: &mut LongModeState,
    code: &mut Code,
    set: &Cpl0ProfileSet<'_>,
    port: &mut Port,
    budget: u64,
    refuse_at: Option<usize>,
) -> (
    od_isa::execute::Cpl0ProfileSetResult<&'static str>,
    Vec<u64>,
) {
    let mut asked = Vec::new();
    let result = run_with_cpl0_profile_set_and_pio_batched(
        state,
        code,
        config(budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        port,
        || false,
        &mut |state: &LongModeState, _: &mut Code| {
            asked.push(state.rip);
            refuse_at != Some(asked.len())
        },
    )
    .expect("admitted");
    (result, asked)
}

/// The same instructions one call each, as the board loop made them before PERF-25: `count`
/// one-instruction calls (each with its own admission); the last result.
fn one_by_one(
    state: &mut LongModeState,
    code: &mut Code,
    set: &Cpl0ProfileSet<'_>,
    port: &mut Port,
    count: u64,
) -> od_isa::execute::Cpl0ProfileSetResult<&'static str> {
    let mut last = None;
    let mut defined = od_isa::INTEGER_FLAGS;
    for _ in 0..count {
        let result = run_with_cpl0_profile_set_and_pio(
            state,
            code,
            config(1),
            defined,
            set,
            None,
            port,
            || false,
        )
        .expect("admitted");
        defined = result.rflags_defined;
        let retired = result.instructions_executed;
        last = Some(result);
        if retired == 0 {
            break;
        }
    }
    last.expect("at least one call")
}

#[test]
fn the_check_is_asked_before_every_further_instruction_and_never_at_the_budget() {
    let mut code = Code::new(vec![0x90; 8]);
    let mut state = state(0);
    let (result, asked) = batched(
        &mut state,
        &mut code,
        &base(),
        &mut Port::default(),
        3,
        None,
    );
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    );
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(
        asked,
        [RIP + 1, RIP + 2],
        "not before the first, not at the budget"
    );
    assert_eq!(state.rip, RIP + 3);
}

#[test]
fn a_refusing_check_ends_the_call_right_after_the_instruction_before_it() {
    let mut code = Code::new(vec![0x90; 8]);
    let mut state = state(0);
    let (result, asked) = batched(
        &mut state,
        &mut code,
        &base(),
        &mut Port::default(),
        8,
        Some(2),
    );
    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(RunExit::StopRequested));
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(asked, [RIP + 1, RIP + 2]);
    assert_eq!(state.rip, RIP + 2, "nothing of the third instruction ran");
}

/// One case of the event table: the code, the set, the entry state, how many instructions the
/// first batched call retires and with which exit.
struct Event {
    name: &'static str,
    code: Vec<u8>,
    set: Cpl0ProfileSet<'static>,
    rflags: u64,
    /// Registers set at the entry (RDX is 0x80 and RSP is `STACK` unless listed).
    regs: Vec<(Register, u64)>,
    cr3: Option<u64>,
    port: Option<u16>,
    /// A last change of the entry state (segments, GDT).
    setup: fn(&mut LongModeState),
    /// Qwords put into the data area (the stack frame, the GDT).
    puts: Vec<(u64, u64)>,
    /// The page that answers a page fault.
    fault_page: Option<u64>,
    /// The MSR cells of the set with the value they have at every entry (the set shares them
    /// between the batched and the reference run).
    cells: Vec<(&'static Cell<u64>, u64)>,
    retired: u64,
    exit: Cpl0ProfileSetExit<&'static str>,
}

fn event(
    name: &'static str,
    code: Vec<u8>,
    set: Cpl0ProfileSet<'static>,
    retired: u64,
    exit: Cpl0ProfileSetExit<&'static str>,
) -> Event {
    Event {
        name,
        code,
        set,
        rflags: 0,
        regs: Vec::new(),
        cr3: None,
        port: None,
        setup: |_| {},
        puts: Vec::new(),
        fault_page: None,
        cells: Vec::new(),
        retired,
        exit,
    }
}

/// `before` NOPs, `op`, four NOPs.
fn nops(before: usize, op: &[u8]) -> Vec<u8> {
    let mut code = vec![0x90; before];
    code.extend_from_slice(op);
    code.extend_from_slice(&[0x90; 4]);
    code
}

fn leak_cell(value: u64) -> &'static Cell<u64> {
    Box::leak(Box::new(Cell::new(value)))
}

fn gp0() -> Cpl0ProfileSetExit<&'static str> {
    Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    }))
}

/// The run reached the end of the code: the exit of a call that nothing ended before.
fn end_of_code(length: usize) -> Cpl0ProfileSetExit<&'static str> {
    Cpl0ProfileSetExit::Run(RunExit::MemoryError(MemoryError::Unmapped {
        address: RIP + length as u64,
        access: AccessKind::Fetch,
    }))
}

static SYSENTER: MsrProfile = MsrProfile::SysenterProbeV1;
static EFER: MsrProfile = MsrProfile::EferReadOnlyV1(0xd01);

const EFER_MSR: u64 = 0xc000_0080;
const RF: u64 = 1 << 16;
const TF: u64 = 1 << 8;
const AC: u64 = 1 << 18;
const STAR: u64 = 0x0023_0010_0000_0000;
const LSTAR: u64 = 0xffff_ffff_8100_0080;

/// CPL0 segments over a GDT with kernel code (0x08), kernel data (0x10), user code (0x1b) and
/// user data (0x23): what `IRETQ`, `MOV SS` and `SYSRETQ` need.
fn kernel_segments(state: &mut LongModeState) {
    state.cs_selector = Some(0x08);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 0xa,
    });
    state.ss_selector = Some(0x10);
    state.ss_cache = Some(DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x93,
        flags: 0xc,
    });
    state.gdtr = Some(Gdtr {
        base: GDT,
        limit: 0x27,
    });
}

fn gdt() -> Vec<(u64, u64)> {
    vec![
        (GDT + 0x08, 0x00af_9b00_0000_ffff),
        (GDT + 0x10, 0x00cf_9300_0000_ffff),
        (GDT + 0x18, 0x00af_fa00_0000_ffff),
        (GDT + 0x20, 0x00cf_f200_0000_ffff),
    ]
}

/// The frame `[RIP][CS][RFLAGS][RSP][SS]` at `STACK` and the GDT.
fn frame(rip: u64, cs: u64, rflags: u64, rsp: u64, ss: u64) -> Vec<(u64, u64)> {
    let mut puts = gdt();
    puts.extend([
        (STACK, rip),
        (STACK + 8, cs),
        (STACK + 16, rflags),
        (STACK + 24, rsp),
        (STACK + 32, ss),
    ]);
    puts
}

fn events() -> Vec<Event> {
    let stop = Cpl0ProfileSetExit::Run(RunExit::StopRequested);
    let mut events = vec![
        // IF 1 -> 0: the interrupt boundary of the caller sees a new IF.
        Event {
            rflags: IF,
            ..event(
                "cli",
                nops(2, &[0xfa]),
                base().with_cli(&CLI),
                3,
                stop.clone(),
            )
        },
        // IF 0 -> 1 and the STI shadow opens.
        event(
            "sti",
            nops(1, &[0xfb]),
            base().with_sti(&STI),
            2,
            stop.clone(),
        ),
        // HLT ends the call itself (halted).
        event(
            "hlt",
            nops(3, &[0xf4]),
            base().with_halt(&HALT),
            4,
            Cpl0ProfileSetExit::Run(RunExit::Halted),
        ),
        // A CPU-owned MOV CR3 ends the call with its translation context change.
        Event {
            regs: vec![(Register::Rax, 0x2000)],
            cr3: Some(0x1000),
            ..event(
                "mov cr3",
                nops(1, &[0x0f, 0x22, 0xd8]),
                base().with_cpu_owned_cr3(MovCr3Long64V1),
                2,
                Cpl0ProfileSetExit::TranslationContextChange(TranslationContextChange::Cr3Write {
                    old_cr3: 0x1000,
                    new_cr3: 0x2000,
                }),
            )
        },
        // A failing port write ends the call with the error; the OUT did not retire.
        Event {
            port: Some(0x80),
            ..event(
                "port error",
                nops(2, &[0xee]),
                base().with_pio_u8(),
                2,
                Cpl0ProfileSetExit::PortIo("write failed"),
            )
        },
        // A memory error of the third instruction (an unmapped read) ends the call before it.
        event(
            "memory error",
            nops(2, &[0x8b, 0x04, 0x25, 0x00, 0x00, 0x00, 0x01]),
            base(),
            2,
            Cpl0ProfileSetExit::Run(RunExit::MemoryError(MemoryError::Unmapped {
                address: 0x0100_0000,
                access: AccessKind::Read,
            })),
        ),
        // A page fault of the third instruction ends the call before it, with the fault.
        Event {
            fault_page: Some(FAULT_PAGE),
            ..event(
                "page fault",
                nops(2, &[0x8b, 0x04, 0x25, 0x00, 0xf0, 0x00, 0x00]),
                base(),
                2,
                Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(FAULT_PAGE),
                })),
            )
        },
        // #GP(0) of an RDMSR of an MSR the profile faults (SYSENTER_CS).
        Event {
            regs: vec![(Register::Rcx, 0x174)],
            ..event(
                "rdmsr gp0",
                nops(2, &[0x0f, 0x32]),
                base().with_msr(&SYSENTER),
                2,
                gp0(),
            )
        },
    ];

    // A committed IRETQ to CPL3 ends the call with the entry; the user code is never fetched.
    let iretq = [0x48, 0xcf];
    events.push(Event {
        setup: kernel_segments,
        puts: frame(USER_RIP, 0x1b, 0x202, 0x7fff_ffff_e000, 0x23),
        ..event(
            "iretq to cpl3",
            nops(2, &iretq),
            base().with_user_mode(),
            3,
            Cpl0ProfileSetExit::UserModeEntered(IretqUserEntry {
                rip: USER_RIP,
                cs: 0x1b,
                rflags: 0x202,
                rsp: 0x7fff_ffff_e000,
                ss: 0x23,
            }),
        )
    });

    // A committed SYSRETQ to CPL3 ends the call the same way.
    let efer = leak_cell(0xd01);
    let star = leak_cell(STAR);
    let lstar = leak_cell(LSTAR);
    let mask = leak_cell(0x25_7fd5);
    events.push(Event {
        regs: vec![(Register::Rcx, 0x40_00c3), (Register::R11, 0x202)],
        setup: kernel_segments,
        cells: vec![
            (efer, 0xd01),
            (star, STAR),
            (lstar, LSTAR),
            (mask, 0x25_7fd5),
        ],
        ..event(
            "sysretq to cpl3",
            nops(2, &[0x48, 0x0f, 0x07]),
            base()
                .with_msr_efer_write(efer)
                .with_msr_star_write(star)
                .with_msr_lstar_write(lstar)
                .with_msr_syscall_mask_write(mask)
                .with_syscall(),
            3,
            Cpl0ProfileSetExit::UserModeEntered(IretqUserEntry {
                rip: 0x40_00c3,
                cs: 0x33,
                rflags: 0x202,
                rsp: STACK,
                ss: 0x2b,
            }),
        )
    });

    // A WRMSR of EFER that changes NXE ends the call with the walker effect.
    let efer_set = |cell: &'static Cell<u64>| {
        base()
            .with_msr(&EFER)
            .with_msr_efer_write(cell)
            .with_cpu_owned_cr3(MovCr3Long64V1)
    };
    let efer = leak_cell(0xd01);
    events.push(Event {
        regs: vec![
            (Register::Rax, 0x501),
            (Register::Rcx, EFER_MSR),
            (Register::Rdx, 0),
        ],
        cr3: Some(0x1000),
        cells: vec![(efer, 0xd01)],
        ..event(
            "wrmsr efer nxe",
            nops(1, &[0x0f, 0x30]),
            efer_set(efer),
            2,
            Cpl0ProfileSetExit::TranslationContextChange(TranslationContextChange::EferWrite {
                old_efer: 0xd01,
                new_efer: 0x501,
            }),
        )
    });
    // Control: an SCE-only change is not an event, the run goes on to the end of the code.
    let efer = leak_cell(0xd01);
    let length = nops(1, &[0x0f, 0x30]).len();
    events.push(Event {
        regs: vec![
            (Register::Rax, 0xd00),
            (Register::Rcx, EFER_MSR),
            (Register::Rdx, 0),
        ],
        cr3: Some(0x1000),
        cells: vec![(efer, 0xd01)],
        ..event(
            "wrmsr efer sce only",
            nops(1, &[0x0f, 0x30]),
            efer_set(efer),
            6, /* NOP, WRMSR, four NOPs */
            end_of_code(length),
        )
    });

    // POPFQ: the flags the caller's boundary looks at (IF, AC) end the call right after it;
    // TF is refused by the set; the arithmetic flags alone are not an event.
    let popf = |name: &'static str, entry: u64, popped: u64, retired: u64, exit| Event {
        rflags: entry,
        regs: vec![(Register::Rsp, STACK)],
        puts: vec![(STACK, popped)],
        ..event(
            name,
            nops(2, &[0x9d]),
            base().with_popf_cpl0(),
            retired,
            exit,
        )
    };
    events.push(popf("popf if 1 to 0", IF, 0x2, 3, stop.clone()));
    events.push(popf("popf if 0 to 1", 0, 0x202, 3, stop.clone()));
    events.push(popf("popf ac 0 to 1", 0, 0x2 | AC, 3, stop.clone()));
    events.push(popf("popf ac 1 to 0", AC, 0x2, 3, stop.clone()));
    events.push(popf(
        "popf tf",
        0,
        0x2 | TF,
        2,
        Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x9d }),
    ));
    let length = nops(2, &[0x9d]).len();
    events.push(popf(
        "popf status flags only",
        0,
        0x2 | 0x41,
        7, // two NOPs, POPFQ, four NOPs
        end_of_code(length),
    ));

    // MOV SS opens the shadow: the caller's interrupt boundary must see it.
    events.push(Event {
        setup: kernel_segments,
        ..event("mov ss", nops(2, &[0x8e, 0xd0]), base(), 3, stop.clone())
    });

    // MOV CR0 / MOV CR4 change what the admission of the next instruction rests on.
    events.push(Event {
        regs: vec![(Register::Rax, 0x8005_0033)],
        ..event(
            "mov cr0",
            nops(2, &[0x0f, 0x22, 0xc0]),
            base(),
            3,
            stop.clone(),
        )
    });
    events.push(Event {
        regs: vec![(Register::Rax, 0x30)],
        ..event(
            "mov cr4",
            nops(2, &[0x0f, 0x22, 0xe0]),
            base(),
            3,
            stop.clone(),
        )
    });

    // A partial REP ends the call as the one-instruction call does: the instruction did not
    // retire and goes on in the next call.
    let partial = Cpl0ProfileSetExit::Run(RunExit::RepIterationBudgetExhausted);
    let rep = |name: &'static str, op: &[u8]| Event {
        regs: vec![
            (Register::Rcx, 1025),
            (Register::Rdi, REP_DST),
            (Register::Rsi, REP_SRC),
        ],
        ..event(name, nops(2, op), base(), 2, partial.clone())
    };
    events.push(rep("rep stosb partial", &[0xf3, 0xaa]));
    events.push(rep("rep stosq partial", &[0xf3, 0x48, 0xab]));
    events.push(rep("rep movsb partial", &[0xf3, 0xa4]));

    // IRETQ to CPL0: RF in the frame is a new RFLAGS the caller sees; without it the return is
    // no event and the run goes on at the target.
    let target = RIP + 3;
    let iretq_cpl0 = |name: &'static str, rflags: u64, retired: u64, exit| Event {
        setup: kernel_segments,
        puts: frame(target, 0x08, rflags, STACK + 0x100, 0x10),
        ..event(name, nops(1, &iretq), base(), retired, exit)
    };
    events.push(iretq_cpl0("iretq rf", 0x2 | RF, 2, stop.clone()));
    let length = nops(1, &iretq).len();
    events.push(iretq_cpl0(
        "iretq no rf",
        0x2,
        6, // NOP, IRETQ, four NOPs
        end_of_code(length),
    ));
    events
}

/// Whether the instruction that ended the call retired (so the next call is a different one).
fn ending_instruction_retired(exit: &Cpl0ProfileSetExit<&'static str>) -> bool {
    matches!(
        exit,
        Cpl0ProfileSetExit::Run(RunExit::StopRequested | RunExit::Halted)
            | Cpl0ProfileSetExit::TranslationContextChange(_)
            | Cpl0ProfileSetExit::UserModeEntered(_)
    )
}

#[test]
fn each_event_ends_the_call_exactly_after_its_instruction_like_one_instruction_calls() {
    for event in events() {
        let name = event.name;
        let prepare = || {
            let mut state = state(event.rflags);
            state.gpr[Register::Rdx as usize] = 0x80;
            state.gpr[Register::Rsp as usize] = STACK;
            for (register, value) in &event.regs {
                state.gpr[*register as usize] = *value;
            }
            state.cr3 = event.cr3;
            (event.setup)(&mut state);
            let mut code = Code::new(event.code.clone());
            code.fault_page = event.fault_page;
            for (address, value) in &event.puts {
                code.put(*address, *value);
            }
            for (cell, value) in &event.cells {
                cell.set(*value);
            }
            let port = Port {
                writes: Vec::new(),
                fail: event.port,
            };
            (state, code, port)
        };
        let cells = || {
            event
                .cells
                .iter()
                .map(|(cell, _)| cell.get())
                .collect::<Vec<_>>()
        };
        let (mut state, mut code, mut port) = prepare();
        let (result, _) = batched(&mut state, &mut code, &event.set, &mut port, 64, None);
        let batched_cells = cells();
        assert_eq!(result.exit, event.exit, "{name}");
        assert_eq!(result.instructions_executed, event.retired, "{name}");
        // The same instructions one call each leave the same state and the same last exit.
        let (mut reference, mut reference_code, mut reference_port) = prepare();
        let mut count = event.retired;
        if !ending_instruction_retired(&result.exit) {
            // The instruction that ended the call without retiring is the next call's.
            count += 1;
        }
        let last = one_by_one(
            &mut reference,
            &mut reference_code,
            &event.set,
            &mut reference_port,
            count,
        );
        assert_eq!(state, reference, "{name}");
        assert_eq!(code, reference_code, "{name}: memory");
        assert_eq!(port.writes, reference_port.writes, "{name}");
        assert_eq!(batched_cells, cells(), "{name}: MSR cells");
        if !matches!(result.exit, Cpl0ProfileSetExit::Run(RunExit::StopRequested)) {
            assert_eq!(last.exit, result.exit, "{name}");
        }
    }
}

#[test]
fn the_sti_shadow_ends_the_next_call_after_one_instruction_too() {
    let set = base().with_sti(&STI);
    let mut code = Code::new(vec![0xfb, 0x90, 0x90, 0x90, 0x90]);
    let mut state = state(0);
    let mut port = Port::default();
    let (first, _) = batched(&mut state, &mut code, &set, &mut port, 64, None);
    assert_eq!(first.instructions_executed, 1);
    assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
    // The instruction in the shadow closes it: the caller's interrupt boundary comes right after.
    let (second, asked) = batched(&mut state, &mut code, &set, &mut port, 64, None);
    assert_eq!(second.exit, Cpl0ProfileSetExit::Run(RunExit::StopRequested));
    assert_eq!(second.instructions_executed, 1);
    assert!(asked.is_empty(), "the entry ended the call before asking");
    assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    // Without an event the rest runs in one call.
    let (third, _) = batched(&mut state, &mut code, &set, &mut port, 3, None);
    assert_eq!(
        third.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    );
    assert_eq!(third.instructions_executed, 3);
}

#[test]
fn the_mov_ss_shadow_ends_the_next_call_after_one_instruction_too() {
    let mut code = Code::new(vec![0x8e, 0xd0, 0x90, 0x90, 0x90, 0x90]);
    let mut state = state(0);
    kernel_segments(&mut state);
    let mut port = Port::default();
    let set = base();
    let (first, _) = batched(&mut state, &mut code, &set, &mut port, 64, None);
    assert_eq!(first.exit, Cpl0ProfileSetExit::Run(RunExit::StopRequested));
    assert_eq!(first.instructions_executed, 1);
    assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
    let (second, asked) = batched(&mut state, &mut code, &set, &mut port, 64, None);
    assert_eq!(second.exit, Cpl0ProfileSetExit::Run(RunExit::StopRequested));
    assert_eq!(second.instructions_executed, 1);
    assert!(asked.is_empty(), "the entry ended the call before asking");
    assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    // Without an event the rest runs in one call, up to the end of the code.
    let (third, _) = batched(&mut state, &mut code, &set, &mut port, 64, None);
    assert_eq!(third.exit, end_of_code(6));
    assert_eq!(third.instructions_executed, 3);
}

#[test]
fn a_partial_rep_resumes_in_the_next_call_like_the_one_call_run_does() {
    // 2500 iterations are three calls (1024 iterations each): the partial REP does not retire,
    // the call that completes it goes on over the NOPs behind it. The budget of 64 instructions
    // is not the REP's iteration budget (a fixed 1024 iterations of the executor).
    let code = nops(1, &[0xf3, 0xaa]);
    let length = code.len();
    let prepare = || {
        let mut state = state(0);
        state.gpr[Register::Rcx as usize] = 2500;
        state.gpr[Register::Rdi as usize] = REP_DST;
        state.gpr[Register::Rax as usize] = 0x5a;
        (state, Code::new(code.clone()), Port::default())
    };
    let partial = Cpl0ProfileSetExit::Run(RunExit::RepIterationBudgetExhausted);
    let expected = [(partial.clone(), 1), (partial, 0), (end_of_code(length), 5)];
    let set = base();

    let (mut state, mut code, mut port) = prepare();
    let mut batched_calls = Vec::new();
    for _ in 0..expected.len() {
        let (result, _) = batched(&mut state, &mut code, &set, &mut port, 64, None);
        batched_calls.push((result.exit, result.instructions_executed));
    }
    assert_eq!(batched_calls, expected);

    let (mut reference, mut reference_code, mut reference_port) = prepare();
    let mut reference_calls = Vec::new();
    let mut defined = od_isa::INTEGER_FLAGS;
    for _ in 0..expected.len() {
        let result = run_with_cpl0_profile_set_and_pio(
            &mut reference,
            &mut reference_code,
            config(64),
            defined,
            &set,
            None,
            &mut reference_port,
            || false,
        )
        .expect("admitted");
        defined = result.rflags_defined;
        reference_calls.push((result.exit, result.instructions_executed));
    }
    assert_eq!(reference_calls, expected);
    assert_eq!(state, reference);
    assert_eq!(code, reference_code);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert!(
        code.data[(REP_DST - DATA) as usize..][..2500]
            .iter()
            .all(|&byte| byte == 0x5a)
    );
}
