//! S7-32 slice Q: the architectural delivery of the exceptions the CPU raises, through the IDT of
//! the guest, in the Long64 CPL0 profile set (from CPL0 or CPL3 to a CPL0 handler).
//!
//! Source: Intel® 64 and IA-32 Architectures Software Developer's Manual, combined volumes,
//! order 325462-093US (local copy, text extraction). Vol. 3A chapter 7 "Interrupt and Exception
//! Handling": §7.5 (classes), Table 7-1 (vectors, classes, error codes), §7.12.1.2–§7.12.1.3 (the
//! gate DPL is not checked for an exception; TF, VM, RF, NT and, for an interrupt gate, IF are
//! cleared), §7.13 (the error code: EXT, IDT, TI and the selector index), §7.14 (64-bit mode: the
//! 16-byte gates, the frame of five or six qwords always with SS:RSP, RSP aligned down to 16, null
//! SS on a CPL change, IST), §7.15 "Event 8" (Tables 7-4 and 7-5: the double fault and shutdown);
//! Vol. 3B §20.3.1.1 (the RF value in the frame); Vol. 2A "INT n/INTO/INT3/INT1—Call to Interrupt
//! Procedure" (the order of the checks and the exception each one raises in IA-32e mode).
//!
//! The shared steps are those of the per-event deliveries of [`super`]: [`super::inspect_cpl0_gate`]
//! (the gate and the target code descriptor), [`super::read_tss_rsp0`] / [`super::read_tss_ist`]
//! (the new stack) and [`super::commit_cpl0_gate`] (the frame, the Accessed bit, the commit). What
//! this module adds is the architectural answer to a check that fails: the exception the CPU
//! raises instead, delivered serially, as a double fault, or as shutdown (triple fault).

use super::{
    GateAbortPhase, GateAborted, GateFailure, GateRefusal, GateRefused, PageFaultFrameSlot,
    TargetCodeFault, TssStackFailure, commit_cpl0_gate, delivery_source, fault_shadow_admitted,
    inspect_cpl0_gate, read_tss_ist, read_tss_rsp0,
};
use crate::LongModeState;
use crate::long_decode::canonical;
use crate::memory::{AccessContext, ArchitecturalFault, GuestMemory, MemoryError};

/// RFLAGS.RF (bit 16).
const RFLAGS_RF: u64 = 1 << 16;

/// The EXT bit (bit 0) of an error code (Vol. 3A §7.13): set for an exception raised while the CPU
/// delivers an event external to the program — an interrupt or an earlier exception. Every event
/// this module delivers is a processor-detected exception, so every exception raised while
/// delivering one has EXT = 1 (the software `INT n`/`INT3`/`INTO` with EXT = 0 are not delivered
/// here).
const EXT: u32 = 1;

/// How an exception is reported (Vol. 3A §7.5, Table 7-1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionClass {
    /// Reported before the instruction completes: the state is the one before the instruction,
    /// the saved RIP points to it, and the RFLAGS image of the frame has RF = 1.
    Fault,
    /// Reported after the instruction completed: the saved RIP points to the next instruction,
    /// the RFLAGS image has RF as it is.
    Trap,
    /// Not restartable (`#DF`, `#MC`): the saved RIP is the boundary RIP of the event, the RFLAGS
    /// image has RF as it is.
    Abort,
}

/// Table 7-1 in IA-32e mode for one vector: whether it can be a fault or an abort (the rest are
/// traps) and whether it pushes an error code. `None` for NMI (vector 2, an interrupt), the reserved
/// 9 (coprocessor segment overrun, not generated after the Intel386) and 15, and 22..=255.
#[derive(Clone, Copy)]
struct VectorEntry {
    fault: bool,
    abort: bool,
    error_code: bool,
}

const fn vector_entry(vector: u8) -> Option<VectorEntry> {
    let (fault, abort, error_code) = match vector {
        // #DE, #BR, #UD, #NM, #MF, #XM, #VE.
        0 | 5 | 6 | 7 | 16 | 19 | 20 => (true, false, false),
        // #DB: a fault (instruction breakpoint, general detect) or a trap (single step, data or
        // I/O breakpoint, task switch, INT1), Table 7-3.
        1 => (true, false, false),
        // #BP (INT3), #OF (INTO): traps.
        3 | 4 => (false, false, false),
        // #DF: an abort with the error code 0.
        8 => (false, true, true),
        // #TS, #NP, #SS, #GP, #PF, #AC, #CP.
        10..=14 | 17 | 21 => (true, false, true),
        // #MC.
        18 => (false, true, false),
        _ => return None,
    };
    Some(VectorEntry {
        fault,
        abort,
        error_code,
    })
}

/// The class of an event for the double-fault rule (Table 7-4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DoubleFaultClass {
    /// Vectors 1–7, 9, 16–19, every `INT n` and every external interrupt.
    Benign,
    /// `#DE`, `#TS`, `#NP`, `#SS`, `#GP`, `#CP`.
    Contributory,
    /// `#PF`, `#VE`.
    PageFault,
    /// `#DF` itself (the last row of Table 7-5).
    DoubleFault,
}

const fn double_fault_class(vector: u8) -> DoubleFaultClass {
    match vector {
        0 | 10..=13 | 21 => DoubleFaultClass::Contributory,
        14 | 20 => DoubleFaultClass::PageFault,
        8 => DoubleFaultClass::DoubleFault,
        _ => DoubleFaultClass::Benign,
    }
}

/// What Table 7-5 makes of a second exception raised while the CPU delivers the first one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SecondException {
    /// "Handle Exceptions Serially": the second exception is delivered (the first is lost; the
    /// instruction raises it again after the handler returns).
    Serial,
    /// "Generate a Double Fault".
    DoubleFault,
    /// "Enter Shutdown Mode" (a contributory or page fault while delivering `#DF`).
    Shutdown,
}

const fn second_exception(first: u8, second: u8) -> SecondException {
    use DoubleFaultClass::{Benign, Contributory, DoubleFault, PageFault};
    match (double_fault_class(first), double_fault_class(second)) {
        (_, Benign) | (Benign, _) | (Contributory, PageFault) => SecondException::Serial,
        (Contributory, Contributory) | (PageFault, Contributory | PageFault) => {
            SecondException::DoubleFault
        }
        (DoubleFault, Contributory | PageFault) => SecondException::Shutdown,
        // A delivery never raises #DF itself; a #DF "second" is handled like a contributory one.
        (Contributory | PageFault, DoubleFault) => SecondException::DoubleFault,
        (DoubleFault, DoubleFault) => SecondException::Shutdown,
    }
}

/// One exception the CPU raised, ready for its delivery through the IDT
/// ([`deliver_exception_cpl0_set`]).
///
/// The event has no RIP of its own: the delivery saves `state.rip` of the boundary it is called at
/// (S7-32 slice Q review: one precondition instead of a second RIP that could disagree with the
/// state and with the nested exceptions, which always use the boundary). For a fault the state is
/// the one before the faulting instruction (RIP on it), for a trap the one after the trapping
/// instruction (RIP of the next one), for an abort the boundary where it is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuException {
    vector: u8,
    error_code: Option<u32>,
    /// `#PF` only: the faulting linear address, CR2 after the delivery.
    page_fault_address: Option<u64>,
    class: ExceptionClass,
}

impl CpuException {
    /// A fault-class exception of the instruction at `state.rip`, which did not retire (the exit
    /// `RunExit::Fault` of the set: the state is the one before the instruction). Table 7-1
    /// decides: the vector must have the fault class and the error code must be present exactly
    /// for the vectors that push one; a `#PF` needs its linear address. `#DB` is refused here: its
    /// class (and its RF value, Vol. 3B §20.3.1.1) depends on the debug condition, which an
    /// `ArchitecturalFault` does not carry.
    pub fn fault(fault: &ArchitecturalFault) -> Result<Self, Cpl0ExceptionRefusal> {
        let vector = fault.vector;
        let entry = vector_entry(vector).ok_or(Cpl0ExceptionRefusal::NotAnException(vector))?;
        if !entry.fault || vector == 1 {
            return Err(Cpl0ExceptionRefusal::ClassMismatch(vector));
        }
        if entry.error_code != fault.error_code.is_some() {
            return Err(Cpl0ExceptionRefusal::ErrorCodeMismatch(vector));
        }
        let page_fault_address = if vector == 14 {
            Some(
                fault
                    .linear_address
                    .ok_or(Cpl0ExceptionRefusal::MissingFaultMetadata)?,
            )
        } else {
            None
        };
        Ok(Self {
            vector,
            error_code: fault.error_code,
            page_fault_address,
            class: ExceptionClass::Fault,
        })
    }

    /// A trap-class debug exception (`#DB`: single step, a data or I/O breakpoint) of the
    /// instruction that completed: `state.rip` is the next instruction, the saved RIP (Vol. 3A
    /// §7.15 "Event 1"). The breakpoint `#BP` (`INT3`) and the overflow `#OF` (`INTO`) are software
    /// interrupts with their own gate DPL check and `EXT = 0` (Vol. 2A "INT n"): they are delivered
    /// by the `INT3` and `INT n` paths of the set (`deliver_int3_cpl0_set`,
    /// `deliver_software_interrupt_cpl0_set`); `INTO` is `#UD` in 64-bit mode.
    pub const fn debug_trap() -> Self {
        Self {
            vector: 1,
            error_code: None,
            page_fault_address: None,
            class: ExceptionClass::Trap,
        }
    }

    /// An abort-class exception (`#MC`, vector 18) at the boundary `state.rip`. `#DF` is not made
    /// here: it is the result of Table 7-5 inside the delivery.
    pub fn abort(vector: u8) -> Result<Self, Cpl0ExceptionRefusal> {
        let entry = vector_entry(vector).ok_or(Cpl0ExceptionRefusal::NotAnException(vector))?;
        if !entry.abort || vector == 8 {
            return Err(Cpl0ExceptionRefusal::ClassMismatch(vector));
        }
        Ok(Self {
            vector,
            error_code: None,
            page_fault_address: None,
            class: ExceptionClass::Abort,
        })
    }

    /// An exception raised while the CPU delivered another event: a fault of the instruction at the
    /// boundary, with the error code the failed check names (Vol. 3A §7.15: the saved RIP of a
    /// `#GP`, `#NP`, `#TS`, `#SS` or `#PF` of a delivery points to the instruction that caused the
    /// delivery).
    const fn raised(vector: u8, error_code: u32, page_fault_address: Option<u64>) -> Self {
        Self {
            vector,
            error_code: Some(error_code),
            page_fault_address,
            class: ExceptionClass::Fault,
        }
    }

    /// `#DF` with the error code 0 ("Event 8": "The processor always pushes an error code of 0");
    /// the saved RIP is undefined by the SDM, the model keeps the boundary RIP.
    const DOUBLE_FAULT: Self = Self {
        vector: 8,
        error_code: Some(0),
        page_fault_address: None,
        class: ExceptionClass::Abort,
    };

    pub const fn vector(&self) -> u8 {
        self.vector
    }

    pub const fn error_code(&self) -> Option<u32> {
        self.error_code
    }

    pub const fn class(&self) -> ExceptionClass {
        self.class
    }

    /// The RFLAGS image of the frame (Vol. 3B §20.3.1.1): RF = 1 for a fault-class exception
    /// (the model raises no instruction-breakpoint `#DB`), RF as it is for a trap and an abort;
    /// bit 1 is always 1.
    const fn saved_rflags(&self, rflags: u64) -> u64 {
        match self.class {
            ExceptionClass::Fault => rflags | RFLAGS_RF | 2,
            ExceptionClass::Trap | ExceptionClass::Abort => rflags | 2,
        }
    }

    const fn nested(&self) -> NestedException {
        NestedException {
            vector: self.vector,
            error_code: self.error_code,
            linear_address: self.page_fault_address,
        }
    }
}

/// An exception the delivery attempted after the first one: one raised by a failed check or
/// access of a delivery (`#GP`, `#NP`, `#TS`, `#SS`, `#PF`), or the `#DF` that Table 7-5 made of
/// two of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NestedException {
    pub vector: u8,
    pub error_code: Option<u32>,
    /// `#PF` only: the linear address of the failed access.
    pub linear_address: Option<u64>,
}

/// Why [`deliver_exception_cpl0_set`] did not deliver: a form outside the model, never an
/// architectural outcome. The CPU state is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0ExceptionRefusal {
    /// The vector is not an exception of Table 7-1 (NMI, reserved, 22..=255).
    NotAnException(u8),
    /// The vector does not have the class of the constructor (for example `#BP` as a fault; `#DB`
    /// as a fault, whose class depends on the debug condition).
    ClassMismatch(u8),
    /// The error code is missing for a vector that pushes one, or present for one that does not.
    ErrorCodeMismatch(u8),
    /// A `#PF` without its linear address.
    MissingFaultMetadata,
    /// The caller's `#PF` record does not belong to the instruction at RIP.
    FaultRipMismatch,
    /// The state and context are not Long64 at CPL0, or at CPL3 under a CS with RPL 3.
    NotRing0Long64,
    /// The CPU is not `Running`.
    ActivityOutsideProfile,
    /// An interrupt shadow the model does not deliver through (see `fault_shadow_admitted`).
    StiShadowActive,
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    MissingIdtr,
    MissingGdtr,
    /// The span of the gate is not canonical (an IDT base at the end of the lower half).
    NonCanonicalIdtSpan,
    /// A reserved bit of the gate is set (byte 4 bits 7:3 or the dword at byte 12); refused
    /// conservatively (the pseudocode does not check them).
    GateReservedBitsSet,
    /// The gate selector has TI = 1: the model has no LDT.
    LdtTargetSelector,
    /// The gate selector has RPL != 0 (ignored by the CPU on an implicit call, §7.12.1.2).
    TargetSelectorRpl,
    /// The span of the target descriptor is not canonical.
    NonCanonicalGdtSpan,
    /// The target code segment is conforming, or a nonconforming one with DPL 1..=3 (the handler
    /// would run at CPL 1..=3): the model runs handlers at CPL0 only.
    UnsupportedTargetCodeSegment,
    /// No task register is modelled (no `LTR`): `TSS.RSP0` or `TSS.IST[n]` is unknown.
    MissingTaskRegister,
    /// The span of the TSS field is not canonical (a TR base at the end of the lower half).
    NonCanonicalTssSpan,
    /// The frame does not fit below the new stack pointer without wrapping below address 0.
    StackAddressOverflow,
}

/// Where a delivery of [`deliver_exception_cpl0_set`] failed on a memory access that is not an
/// architectural `#PF`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0ExceptionPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    /// The read of `TSS.RSP0` (from CPL3) or `TSS.IST[n]` (an IST gate).
    TssStackRead,
    StackWrite {
        slot: PageFaultFrameSlot,
    },
    TargetCodeDescriptorAccessedWrite,
}

/// The result of [`deliver_exception_cpl0_set`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cpl0ExceptionDelivery {
    /// The CPU entered the handler of `vector` — the first exception, or what Table 7-5 made of
    /// it (`nested` is then not empty). The frame is on the new stack; CS, RIP, RSP, RFLAGS (and
    /// SS from CPL3) are those of the handler; CR2 holds the address of the last `#PF` of the
    /// chain, if there was one. Nothing retired.
    Delivered {
        vector: u8,
        target_rip: u64,
        new_rsp: u64,
        trap_gate: bool,
        nested: Vec<NestedException>,
    },
    /// A contributory exception or a page fault while delivering `#DF`: the CPU entered shutdown
    /// (`activity = Shutdown`; "Enter Shutdown Mode", Table 7-5). No handler was entered: RIP,
    /// RSP, CS and RFLAGS are those before the first exception; CR2 holds the address of the last
    /// `#PF` of the chain. Only NMI, INIT or reset leave this state (SMP_CONTRACT.md); the model does
    /// not record whether the shutdown happened in an NMI handler (`hardware_reset_only`).
    Shutdown { nested: Vec<NestedException> },
    /// A form outside the model while delivering `vector` (the first exception, or the last of
    /// `nested`). The CPU state is unchanged; frame words a failed earlier attempt of the chain
    /// stored stay below that attempt's stack (dead stack, as on hardware).
    Refused {
        vector: u8,
        reason: Cpl0ExceptionRefusal,
        nested: Vec<NestedException>,
    },
    /// A memory access of the delivery of `vector` failed with an error that is not an
    /// architectural `#PF` (a backend or host error). The CPU state is unchanged; frame words
    /// already stored stay below the stack of the attempt.
    Aborted {
        vector: u8,
        phase: Cpl0ExceptionPhase,
        cause: MemoryError,
        nested: Vec<NestedException>,
    },
}

/// How one attempt of a delivery failed.
enum AttemptFailure {
    /// A check or an access raised this exception instead (a fault of the boundary).
    Raise(CpuException),
    Refused(Cpl0ExceptionRefusal),
    Aborted {
        phase: Cpl0ExceptionPhase,
        cause: MemoryError,
    },
}

/// The error code of a selector (§7.13): the index and TI of the selector, IDT = 0, EXT = 1.
const fn selector_error_code(selector: u16) -> u32 {
    (selector as u32 & 0xfffc) | EXT
}

/// The error code of an IDT vector (§7.13): `vector * 8`, IDT = 1, EXT = 1.
const fn idt_error_code(vector: u8) -> u32 {
    vector as u32 * 8 + 2 + EXT
}

/// A failed access of the delivery: an architectural `#PF` (with its error code and linear
/// address) is raised; anything else ends the delivery as `Aborted` at `phase`.
fn memory_failure(cause: MemoryError, phase: Cpl0ExceptionPhase) -> AttemptFailure {
    match cause {
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(error_code),
            linear_address: Some(address),
        }) => AttemptFailure::Raise(CpuException::raised(14, error_code, Some(address))),
        cause => AttemptFailure::Aborted { phase, cause },
    }
}

/// What the CPU does when a check of the gate or of the target code descriptor fails while it
/// delivers `vector` (the IA-32e branch and `TRAP-OR-INTERRUPT-GATE` of the "INT n" pseudocode;
/// §7.14.1 for the 64-bit code segment and the canonical handler RIP).
fn gate_failure(failure: GateFailure, vector: u8) -> AttemptFailure {
    use AttemptFailure::Refused;
    use Cpl0ExceptionRefusal as R;
    let raise = |vector: u8, error_code: u32| {
        AttemptFailure::Raise(CpuException::raised(vector, error_code, None))
    };
    let refusal = match failure {
        GateAborted { phase, cause } => {
            let phase = match phase {
                GateAbortPhase::IdtGateRead => Cpl0ExceptionPhase::IdtGateRead,
                GateAbortPhase::TargetCodeDescriptorRead => {
                    Cpl0ExceptionPhase::TargetCodeDescriptorRead
                }
                GateAbortPhase::StackWrite(index) => Cpl0ExceptionPhase::StackWrite {
                    slot: FRAME_SLOTS[index],
                },
                GateAbortPhase::TargetCodeDescriptorAccessedWrite => {
                    Cpl0ExceptionPhase::TargetCodeDescriptorAccessedWrite
                }
            };
            return memory_failure(cause, phase);
        }
        GateRefused(refusal) => refusal,
    };
    match refusal {
        // "not in IDT limits or ... not an interrupt-, or trap-gate type": #GP(vector, IDT, EXT);
        // the type is checked before the present bit (a zero gate is #GP).
        GateRefusal::GateBeyondIdtLimit | GateRefusal::InvalidGateType(_) => {
            raise(13, idt_error_code(vector))
        }
        GateRefusal::GateNotPresent(attributes) if !is_interrupt_or_trap_gate(attributes) => {
            raise(13, idt_error_code(vector))
        }
        // "gate not present": #NP(vector, IDT, EXT).
        GateRefusal::GateNotPresent(_) => raise(11, idt_error_code(vector)),
        // "new code-segment selector is NULL": #GP(EXT), the error code of a null selector.
        GateRefusal::NullTargetSelector => raise(13, EXT),
        // "not within its descriptor table limits", "not a code segment or DPL > CPL":
        // #GP(selector, EXT).
        GateRefusal::TargetBeyondGdtLimit(selector)
        | GateRefusal::InvalidTargetCodeDescriptor(
            TargetCodeFault::NotCode | TargetCodeFault::DplAboveCpl,
            selector,
        ) => raise(13, selector_error_code(selector)),
        // "new code-segment descriptor is not present": #NP(selector, EXT).
        GateRefusal::InvalidTargetCodeDescriptor(TargetCodeFault::NotPresent, selector) => {
            raise(11, selector_error_code(selector))
        }
        // §7.14.1: "If the target is not a 64-bit code segment, a general-protection exception
        // (#GP) is generated with the IDT vector number reported as the error code."
        GateRefusal::InvalidTargetCodeDescriptor(TargetCodeFault::Not64Bit, _) => {
            raise(13, idt_error_code(vector))
        }
        // "instruction pointer from IDT gate contains a non-canonical address": #GP(EXT).
        GateRefusal::NonCanonicalTargetRip => raise(13, EXT),
        // A push to a non-canonical stack address: #SS(EXT) (64-bit gate, "StackAddress is
        // non-canonical").
        GateRefusal::NonCanonicalStackSpan => raise(12, EXT),
        GateRefusal::InvalidTargetCodeDescriptor(TargetCodeFault::Unsupported, _) => {
            Refused(R::UnsupportedTargetCodeSegment)
        }
        GateRefusal::MissingCurrentSelectors => Refused(R::MissingCurrentSelectors),
        GateRefusal::InvalidCurrentCodeSegment => Refused(R::InvalidCurrentCodeSegment),
        GateRefusal::MissingIdtr => Refused(R::MissingIdtr),
        GateRefusal::MissingGdtr => Refused(R::MissingGdtr),
        GateRefusal::NonCanonicalIdtSpan => Refused(R::NonCanonicalIdtSpan),
        GateRefusal::GateReservedBitsSet => Refused(R::GateReservedBitsSet),
        GateRefusal::LdtTargetSelector => Refused(R::LdtTargetSelector),
        GateRefusal::TargetSelectorRpl => Refused(R::TargetSelectorRpl),
        GateRefusal::NonCanonicalGdtSpan => Refused(R::NonCanonicalGdtSpan),
        GateRefusal::StackAddressOverflow => Refused(R::StackAddressOverflow),
        GateRefusal::UnsupportedIst(_) => {
            unreachable!("deliver_exception_cpl0_set inspects gates with IST allowed")
        }
    }
}

/// A 64-bit interrupt gate (0xE) or trap gate (0xF) with S = 0, whatever its present bit.
const fn is_interrupt_or_trap_gate(attributes: u8) -> bool {
    attributes & 0x1e == 0x0e
}

/// The frame slots in store order (the index of `GateAbortPhase::StackWrite`).
const FRAME_SLOTS: [PageFaultFrameSlot; 6] = [
    PageFaultFrameSlot::OldSs,
    PageFaultFrameSlot::OldRsp,
    PageFaultFrameSlot::OldRflags,
    PageFaultFrameSlot::OldCs,
    PageFaultFrameSlot::SavedRip,
    PageFaultFrameSlot::ErrorCode,
];

/// One attempt to enter the handler of `exception`, in the order of the "INT n" pseudocode: the
/// gate and the target code descriptor ([`inspect_cpl0_gate`]); the new stack (`TSS.IST[n]` for
/// an IST gate, else `TSS.RSP0` from CPL3, else the current RSP): `#TS(TSS selector, EXT)` when
/// the field lies beyond the TR limit, `#SS(EXT)` for a non-canonical new RSP (from the TSS, and
/// since the review of S7-32 slice Q also the current RSP of `INTRA-PRIVILEGE-LEVEL-INTERRUPT`,
/// "IF NewRSP contains a non-canonical address THEN #SS(EXT)", which comes before the RIP check);
/// the canonical handler RIP (`#GP(EXT)`); then the frame stores and the Accessed bit
/// ([`commit_cpl0_gate`]), where a `#PF` of a store is raised and a frame that runs into
/// non-canonical addresses is `#SS(EXT)`. The saved RIP is `state.rip`, the boundary. A failure
/// leaves the CPU state unchanged. Returns `(target_rip, new_rsp, trap_gate)`.
fn attempt(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    from_cpl3: bool,
    exception: &CpuException,
) -> Result<(u64, u64, bool), AttemptFailure> {
    let mut gate = inspect_cpl0_gate(state, memory, context, exception.vector, from_cpl3, true)
        .map_err(|failure| gate_failure(failure, exception.vector))?;
    let stack = if gate.ist != 0 {
        Some(read_tss_ist(state, memory, context, gate.ist))
    } else if from_cpl3 {
        Some(read_tss_rsp0(state, memory, context))
    } else {
        None
    };
    if let Some(stack) = stack {
        gate.new_stack = Some(stack.map_err(|failure| match failure {
            TssStackFailure::MissingTaskRegister => {
                AttemptFailure::Refused(Cpl0ExceptionRefusal::MissingTaskRegister)
            }
            // "(TSSstackAddress + 7) > current TSS limit": #TS(current TSS selector, EXT).
            TssStackFailure::BeyondLimit => AttemptFailure::Raise(CpuException::raised(
                10,
                selector_error_code(state.tr.map_or(0, |tr| tr.selector)),
                None,
            )),
            TssStackFailure::NonCanonicalSpan => {
                AttemptFailure::Refused(Cpl0ExceptionRefusal::NonCanonicalTssSpan)
            }
            // "IF NewRSP contains a non-canonical address THEN #SS(EXT)".
            TssStackFailure::NonCanonicalValue => {
                AttemptFailure::Raise(CpuException::raised(12, EXT, None))
            }
            TssStackFailure::Aborted(cause) => {
                memory_failure(cause, Cpl0ExceptionPhase::TssStackRead)
            }
        })?);
    } else if !canonical(state.gpr[crate::Register::Rsp as usize]) {
        // The same check for the current RSP of a CPL0 to CPL0 delivery without IST.
        return Err(AttemptFailure::Raise(CpuException::raised(12, EXT, None)));
    }
    if !canonical(gate.target_rip) {
        return Err(gate_failure(
            GateRefused(GateRefusal::NonCanonicalTargetRip),
            exception.vector,
        ));
    }
    let saved_rflags = exception.saved_rflags(state.rflags);
    let saved_rip = state.rip;
    let (target_rip, new_rsp) = commit_cpl0_gate(
        state,
        memory,
        context,
        &gate,
        saved_rflags,
        saved_rip,
        exception.error_code.map(u64::from),
    )
    .map_err(|failure| gate_failure(failure, exception.vector))?;
    Ok((target_rip, new_rsp, gate.trap_gate))
}

/// Deliver `exception` through the IDT of the guest in the Long64 CPL0 profile set: from CPL0, or
/// from CPL3 through `TSS.RSP0`, to a CPL0 handler of an interrupt gate (0xE) or a trap gate
/// (0xF), with IST 0 or IST 1..=7 (`TSS.IST[n]`, also from CPL3, where it wins over `RSP0`).
///
/// **Entry conditions** (checked once, before any access): the state and context are Long64 at
/// CPL0, or at CPL3 under a CS with RPL 3; the CPU is `Running`; the interrupt shadow is one an
/// exception is delivered in (as for the `#PF` and `#GP` deliveries of the set: `Clear`, or
/// `Blocking` with IF = 1 at CPL0, which the delivery ends, Vol. 2B "STI"). The gate DPL is not
/// checked (§7.12.1.2: "For hardware-generated interrupts and processor-detected exceptions, the
/// processor ignores the DPL of interrupt and trap gates").
///
/// **Frame** (§7.14.2): the new RSP (the current one, `TSS.RSP0` or `TSS.IST[n]`) aligned down to
/// 16, then SS, the old RSP, the RFLAGS image ([`CpuException`]: RF = 1 for a fault), CS, the saved
/// RIP (`state.rip`: the delivery is called at the boundary of the event, see [`CpuException`])
/// and, for the vectors of Table 7-1 that have one, the error code, eight bytes each,
/// downwards. The stores of a delivery from CPL3 are made at CPL0. **Entry** (§7.12.1.3, §7.14.4):
/// CS from the gate (RPL 0), RIP the gate offset, RSP the bottom of the frame, SS kept without a
/// privilege change (also through an IST gate: the `INTRA-PRIVILEGE-LEVEL-INTERRUPT` branch of the
/// pseudocode loads no SS) and null (selector 0 = RPL of CPL0) from CPL3; TF, VM, RF and NT cleared
/// and IF cleared for an interrupt gate (AC kept: the pseudocode clears it only in real-address
/// mode); the interrupt shadow ends; CR2 = the address of the last `#PF` of the chain.
///
/// **A check that fails** is not a stop when the CPU defines what happens: the exception it raises
/// instead ([`gate_failure`], `attempt`) is a second exception, and Table 7-5 decides with the
/// classes of Table 7-4: delivered serially (for example a `#PF` of a frame store while delivering
/// `#DE`: the `#PF` handler runs, the instruction raises `#DE` again after it), a `#DF` (error code
/// 0, IST as its gate says), or shutdown (`activity = Shutdown`). Every exception of the chain has
/// the boundary RIP as its saved RIP and EXT = 1 in its error code. The chain is finite: a second
/// exception is never benign, so after a contributory or page fault the next step is `#DF` or
/// serial into a page fault, and after `#DF` only shutdown or delivery are left.
///
/// What the model does not run (an LDT selector, RPL != 0 of the gate selector, set reserved gate
/// bits, a conforming or non-CPL0 handler segment, no task register, a non-canonical table span, a
/// frame wrapping below 0) is `Refused` with the CPU state unchanged; a memory error that is not
/// an architectural `#PF` is `Aborted`, also with the CPU state unchanged.
pub fn deliver_exception_cpl0_set(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    exception: CpuException,
    context: AccessContext,
) -> Cpl0ExceptionDelivery {
    let refused = |reason| Cpl0ExceptionDelivery::Refused {
        vector: exception.vector,
        reason,
        nested: Vec::new(),
    };
    let Some(from_cpl3) = delivery_source(state, context) else {
        return refused(Cpl0ExceptionRefusal::NotRing0Long64);
    };
    if state.activity != Some(crate::CpuActivity::Running) {
        return refused(Cpl0ExceptionRefusal::ActivityOutsideProfile);
    }
    if !fault_shadow_admitted(state, from_cpl3, false) {
        return refused(Cpl0ExceptionRefusal::StiShadowActive);
    }
    let mut current = exception;
    let mut cr2 = exception.page_fault_address;
    let mut nested = Vec::new();
    loop {
        let second = match attempt(state, memory, context, from_cpl3, &current) {
            Ok((target_rip, new_rsp, trap_gate)) => {
                if cr2.is_some() {
                    state.cr2 = cr2;
                }
                return Cpl0ExceptionDelivery::Delivered {
                    vector: current.vector,
                    target_rip,
                    new_rsp,
                    trap_gate,
                    nested,
                };
            }
            Err(AttemptFailure::Raise(second)) => second,
            Err(AttemptFailure::Refused(reason)) => {
                return Cpl0ExceptionDelivery::Refused {
                    vector: current.vector,
                    reason,
                    nested,
                };
            }
            Err(AttemptFailure::Aborted { phase, cause }) => {
                return Cpl0ExceptionDelivery::Aborted {
                    vector: current.vector,
                    phase,
                    cause,
                    nested,
                };
            }
        };
        // Footnote 1 of "Event 14": CR2 takes the address of every page fault detected, also one
        // that results in a double fault or occurs during the delivery of one.
        if second.page_fault_address.is_some() {
            cr2 = second.page_fault_address;
        }
        nested.push(second.nested());
        current = match second_exception(current.vector, second.vector) {
            SecondException::Serial => second,
            SecondException::DoubleFault => {
                nested.push(CpuException::DOUBLE_FAULT.nested());
                CpuException::DOUBLE_FAULT
            }
            SecondException::Shutdown => {
                state.activity = Some(crate::CpuActivity::Shutdown);
                if cr2.is_some() {
                    state.cr2 = cr2;
                }
                return Cpl0ExceptionDelivery::Shutdown { nested };
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_7_5_pairs() {
        use SecondException::{DoubleFault, Serial, Shutdown};
        // first #UD (benign), #DE (contributory), #PF, #DF; second #GP, #PF.
        let cases = [
            (6, 13, Serial),
            (6, 14, Serial),
            (0, 13, DoubleFault),
            (0, 14, Serial),
            (14, 13, DoubleFault),
            (14, 14, DoubleFault),
            (8, 11, Shutdown),
            (8, 14, Shutdown),
            (20, 12, DoubleFault),
            (21, 10, DoubleFault),
            (17, 13, Serial),
        ];
        for (first, second, expected) in cases {
            assert_eq!(
                second_exception(first, second),
                expected,
                "{first} then {second}"
            );
        }
    }
}
