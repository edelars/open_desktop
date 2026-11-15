//! Bounded caller-selected external interrupt delivery for a private Long64 CPU.

use crate::long_decode::canonical;
use crate::memory::{
    AccessContext, AccessKind, AccessOrigin, GuestMemory, InstructionMode, MemoryError,
    PrivilegeLevel,
};
use crate::state::CodeSegmentDescriptorCache;
use crate::{INTEGER_FLAGS, LongModeState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptDeliveryPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    StackWrite { slot: InterruptFrameSlot },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptFrameSlot {
    OldSs,
    OldRsp,
    OldRflags,
    OldCs,
    OldRip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageFaultFrameSlot {
    OldSs,
    OldRsp,
    OldRflags,
    OldCs,
    SavedRip,
    ErrorCode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageFaultDeliveryPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    TargetCodeDescriptorAccessedPreflight,
    StackWrite { slot: PageFaultFrameSlot },
    TargetCodeDescriptorAccessedWrite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageFaultDeliveryOutsideProfile {
    NotPageFault,
    MissingFaultMetadata,
    NotExplicitInstructionFault,
    FaultRipMismatch,
    MissingPagingState,
    UnsupportedControlState,
    NotRing0Long64,
    ActivityOutsideProfile,
    StiShadowActive,
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    InvalidCurrentStackSegment,
    MissingIdtr,
    MissingIdtEntry,
    NonCanonicalIdtSpan,
    InvalidGate,
    InvalidTargetSelector,
    MissingGdtr,
    MissingTargetDescriptor,
    TargetDescriptorNotAccessed,
    TargetDescriptorAccessedWriteOverlapsFrame,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageFaultDeliveryResult {
    OutsideProfile(PageFaultDeliveryOutsideProfile),
    /// A terminal diagnostic result. If an original #PF encounters a
    /// secondary #PF, architectural delivery requires #DF, which this bounded
    /// API does not model. Any retained frame writes or uncommitted CPU entry
    /// state must not be resumed or retried as though delivery could continue.
    DeliveryAborted {
        phase: PageFaultDeliveryPhase,
        cause: MemoryError,
    },
    Delivered {
        target_rip: u64,
        new_rsp: u64,
    },
}

/// Deliver a synchronous Long64 CPL0 page fault through one same-CPL interrupt
/// gate (vector 14, IST 0). The page-fault CR2 value is committed before table
/// or stack accesses; CPU entry state is committed only after all six qword
/// frame stores and any target-code descriptor Accessed-bit write succeed. The
/// frame-stores-then-descriptor-write order is an explicit bounded emulator
/// profile decision, not a claim about hardware fault-visible ordering. A
/// failed delivery is terminal because prior stores may already have committed;
/// notably, a secondary #PF would require #DF, which this API does not model.
pub fn deliver_page_fault(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    record: &crate::memory::PageFaultRecord,
    context: AccessContext,
) -> PageFaultDeliveryResult {
    deliver_exception(state, memory, record, context, ExceptionKind::PageFault)
}

/// Deliver vector 8 after a secondary #PF interrupted delivery of the original
/// #PF. The original fault RIP is saved solely as a deterministic diagnostic;
/// #DF is an abort and this address must not be used to restart execution.
/// CR2 is not changed by #DF itself. A further page fault during delivery does
/// update CR2 and is returned as an aborted delivery to the caller.
pub fn deliver_double_fault(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    record: &crate::memory::DoubleFaultRecord,
    context: AccessContext,
) -> PageFaultDeliveryResult {
    deliver_exception(
        state,
        memory,
        record.original(),
        context,
        ExceptionKind::DoubleFault,
    )
}

#[derive(Clone, Copy)]
enum ExceptionKind {
    PageFault,
    DoubleFault,
}

fn deliver_exception(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    record: &crate::memory::PageFaultRecord,
    context: AccessContext,
    kind: ExceptionKind,
) -> PageFaultDeliveryResult {
    if record.fault().vector != 14 {
        return pf_outside(PageFaultDeliveryOutsideProfile::NotPageFault);
    }
    if record.fault().error_code != Some(3)
        || record.fault().linear_address != Some(record.cr2_value())
        || record.access() != AccessKind::Write
        || record.context() != context
    {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingFaultMetadata);
    }
    if state.rip != record.saved_rip() {
        return pf_outside(PageFaultDeliveryOutsideProfile::FaultRipMismatch);
    }
    if context.origin != AccessOrigin::Explicit {
        return pf_outside(PageFaultDeliveryOutsideProfile::NotExplicitInstructionFault);
    }
    let (Some(cr0), Some(_cr3), Some(cr4)) = (state.cr0, state.cr3, state.cr4) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingPagingState);
    };
    const CR0_REQUIRED: u64 = (1 << 0) | (1 << 16) | (1 << 31); // PE, WP, PG
    const CR4_PAE: u64 = 1 << 5;
    if cr0 & CR0_REQUIRED != CR0_REQUIRED || cr4 & CR4_PAE == 0 {
        return pf_outside(PageFaultDeliveryOutsideProfile::UnsupportedControlState);
    }
    if context.mode != InstructionMode::Long64 || context.privilege != PrivilegeLevel::Ring0 {
        return pf_outside(PageFaultDeliveryOutsideProfile::NotRing0Long64);
    }
    if state.activity != Some(crate::CpuActivity::Running) {
        return pf_outside(PageFaultDeliveryOutsideProfile::ActivityOutsideProfile);
    }
    if state.sti_shadow != Some(crate::StiShadow::Clear) {
        return pf_outside(PageFaultDeliveryOutsideProfile::StiShadowActive);
    }
    let (Some(old_cs), Some(old_cs_cache), Some(old_ss), Some(old_ss_cache), Some(idtr)) = (
        state.cs_selector,
        state.cs_cache,
        state.ss_selector,
        state.ss_cache,
        state.idtr,
    ) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingCurrentSelectors);
    };
    if !valid_ring0_long_code(old_cs, old_cs_cache.access, old_cs_cache.flags)
        || old_cs_cache.base != 0
        || old_cs_cache.limit != u32::MAX
    {
        return pf_outside(PageFaultDeliveryOutsideProfile::InvalidCurrentCodeSegment);
    }
    if old_ss & 0xfffc == 0
        || old_ss & 3 != 0
        || old_ss_cache.access & 0x10 == 0
        || old_ss_cache.access & 0x08 != 0
        || old_ss_cache.access & 0x02 == 0
        || old_ss_cache.access & 0x60 != 0
        || old_ss_cache.access & 0x80 == 0
    {
        return pf_outside(PageFaultDeliveryOutsideProfile::InvalidCurrentStackSegment);
    }
    if state.gdtr.is_none() {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingGdtr);
    }
    if matches!(kind, ExceptionKind::PageFault) {
        state.cr2 = Some(record.cr2_value());
    }

    let idt_offset = match kind {
        ExceptionKind::PageFault => 14_u64 * 16,
        ExceptionKind::DoubleFault => 8_u64 * 16,
    };
    if idt_offset + 15 > u64::from(idtr.limit) {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingIdtEntry);
    }
    let Some(gate_address) = idtr.base.checked_add(idt_offset) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalIdtSpan);
    };
    let Some(gate_last) = gate_address.checked_add(15) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalIdtSpan);
    };
    if !canonical(gate_address) || !canonical(gate_last) {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalIdtSpan);
    }
    let mut gate = [0; 16];
    if let Err(cause) = memory.read(gate_address, context.descriptor_table(), &mut gate) {
        return pf_delivery_aborted(state, PageFaultDeliveryPhase::IdtGateRead, cause);
    }
    let gate_type = gate[5] & 0x0f;
    if gate[4] & 0xf8 != 0
        || gate[4] & 7 != 0
        || gate[12..16] != [0, 0, 0, 0]
        || gate_type != 0x0e
        || gate[5] & 0x10 != 0
        || gate[5] & 0x80 == 0
    {
        return pf_outside(PageFaultDeliveryOutsideProfile::InvalidGate);
    }
    let target_selector = u16::from_le_bytes([gate[2], gate[3]]);
    if target_selector & 0xfffc == 0 || target_selector & 4 != 0 || target_selector & 3 != 0 {
        return pf_outside(PageFaultDeliveryOutsideProfile::InvalidTargetSelector);
    }
    let Some(gdtr) = state.gdtr else {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingGdtr);
    };
    let descriptor_offset = u64::from(target_selector & !7);
    if descriptor_offset + 7 > u64::from(gdtr.limit) {
        return pf_outside(PageFaultDeliveryOutsideProfile::MissingTargetDescriptor);
    }
    let Some(descriptor_address) = gdtr.base.checked_add(descriptor_offset) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalGdtSpan);
    };
    let Some(descriptor_last) = descriptor_address.checked_add(7) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalGdtSpan);
    };
    if !canonical(descriptor_address) || !canonical(descriptor_last) {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalGdtSpan);
    }
    let mut descriptor = [0; 8];
    if let Err(cause) = memory.read(
        descriptor_address,
        context.descriptor_table(),
        &mut descriptor,
    ) {
        return pf_delivery_aborted(
            state,
            PageFaultDeliveryPhase::TargetCodeDescriptorRead,
            cause,
        );
    }
    let access = descriptor[5];
    let flags = descriptor[6] >> 4;
    let descriptor_base = u32::from(descriptor[2])
        | (u32::from(descriptor[3]) << 8)
        | (u32::from(descriptor[4]) << 16)
        | (u32::from(descriptor[7]) << 24);
    let raw_limit = u32::from(descriptor[0])
        | (u32::from(descriptor[1]) << 8)
        | (u32::from(descriptor[6] & 0x0f) << 16);
    let descriptor_limit = if descriptor[6] & 0x80 != 0 {
        (raw_limit << 12) | 0xfff
    } else {
        raw_limit
    };
    let target_rip = u64::from(u16::from_le_bytes([gate[0], gate[1]]))
        | (u64::from(u16::from_le_bytes([gate[6], gate[7]])) << 16)
        | (u64::from(u32::from_le_bytes(gate[8..12].try_into().unwrap())) << 32);
    if !canonical(target_rip) {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalTargetRip);
    }
    if !valid_ring0_long_code(target_selector, access | 1, flags)
        || descriptor_base != 0
        || descriptor_limit != u32::MAX
    {
        return pf_outside(PageFaultDeliveryOutsideProfile::InvalidTargetCodeDescriptor);
    }

    let old_rsp = state.gpr[crate::Register::Rsp as usize];
    let aligned_rsp = old_rsp & !15;
    let Some(new_rsp) = aligned_rsp.checked_sub(48) else {
        return pf_outside(PageFaultDeliveryOutsideProfile::StackAddressOverflow);
    };
    let mut saved_rflags = state.rflags;
    if matches!(kind, ExceptionKind::PageFault) {
        saved_rflags |= 1 << 16; // RF is set in a fault-class frame.
    }
    if saved_rflags & (1 << 1) == 0 {
        saved_rflags |= 1 << 1;
    }
    let slots = [
        (
            aligned_rsp - 8,
            u64::from(old_ss),
            PageFaultFrameSlot::OldSs,
        ),
        (aligned_rsp - 16, old_rsp, PageFaultFrameSlot::OldRsp),
        (
            aligned_rsp - 24,
            saved_rflags,
            PageFaultFrameSlot::OldRflags,
        ),
        (
            aligned_rsp - 32,
            u64::from(old_cs),
            PageFaultFrameSlot::OldCs,
        ),
        (
            aligned_rsp - 40,
            record.saved_rip(),
            PageFaultFrameSlot::SavedRip,
        ),
        (
            aligned_rsp - 48,
            match kind {
                ExceptionKind::PageFault => u64::from(record.fault().error_code.unwrap()),
                ExceptionKind::DoubleFault => 0,
            },
            PageFaultFrameSlot::ErrorCode,
        ),
    ];
    if slots.iter().any(|(address, _, _)| {
        address
            .checked_add(7)
            .is_none_or(|last| !canonical(*address) || !canonical(last))
    }) {
        return pf_outside(PageFaultDeliveryOutsideProfile::NonCanonicalStackSpan);
    }
    let stack_context = AccessContext {
        origin: AccessOrigin::ImplicitStack,
        ..context
    };
    if access & 1 == 0 {
        let stack_writes = slots.map(|(address, _, _)| (address, 8));
        if let Err(cause) = memory.preflight_descriptor_accessed_write(
            descriptor_address + 5,
            context.descriptor_table(),
            &stack_writes,
        ) {
            return pf_delivery_aborted(
                state,
                PageFaultDeliveryPhase::TargetCodeDescriptorAccessedPreflight,
                cause,
            );
        }
    }
    for (address, value, slot) in slots {
        if let Err(cause) = memory.write(address, stack_context, &value.to_le_bytes()) {
            return pf_delivery_aborted(state, PageFaultDeliveryPhase::StackWrite { slot }, cause);
        }
    }

    // This bounded profile completes the frame first, then performs the target
    // descriptor A-bit write, before committing CPU entry state. This sequence
    // defines emulator-visible partial effects; it does not assert hardware
    // fault-visible ordering.
    let cached_access = access | 1;
    if access & 1 == 0
        && let Err(cause) = memory.write(
            descriptor_address + 5,
            context.descriptor_table(),
            &[cached_access],
        )
    {
        return pf_delivery_aborted(
            state,
            PageFaultDeliveryPhase::TargetCodeDescriptorAccessedWrite,
            cause,
        );
    }

    state.cs_selector = Some(target_selector & !3);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: cached_access,
        flags,
    });
    state.gpr[crate::Register::Rsp as usize] = new_rsp;
    state.rip = target_rip;
    // Interrupt-gate entry clears IF, TF, NT, RF, and VM in the live flags.
    state.rflags &= !((1 << 9) | (1 << 8) | (1 << 17) | (1 << 16) | (1 << 14));
    pf_delivered(target_rip, new_rsp)
}

fn valid_ring0_long_code(selector: u16, access: u8, flags: u8) -> bool {
    selector & 0xfffc != 0
        && selector & 3 == 0
        && access & 0x18 == 0x18
        && access & 0x04 == 0
        && access & 0x60 == 0
        && access & 0x80 != 0
        && access & 1 != 0
        && flags & 0x2 != 0
        && flags & 0x4 == 0
}

/// The current CS of a CPL3 delivery: a present, non-conforming 64-bit code segment with DPL 3
/// under a selector with RPL 3 (the state an `IRETQ` to CPL3 leaves, S4-39a).
fn valid_ring3_long_code(selector: u16, access: u8, flags: u8) -> bool {
    selector & 0xfffc != 0
        && selector & 3 == 3
        && access & 0x18 == 0x18
        && access & 0x04 == 0
        && (access >> 5) & 3 == 3
        && access & 0x80 != 0
        && flags & 0x2 != 0
        && flags & 0x4 == 0
}

fn pf_outside(reason: PageFaultDeliveryOutsideProfile) -> PageFaultDeliveryResult {
    PageFaultDeliveryResult::OutsideProfile(reason)
}

fn pf_delivery_aborted(
    state: &mut LongModeState,
    phase: PageFaultDeliveryPhase,
    cause: MemoryError,
) -> PageFaultDeliveryResult {
    if let MemoryError::Architectural(fault) = &cause
        && fault.vector == 14
        && let Some(address) = fault.linear_address
    {
        state.cr2 = Some(address);
    }
    PageFaultDeliveryResult::DeliveryAborted { phase, cause }
}

fn pf_delivered(target_rip: u64, new_rsp: u64) -> PageFaultDeliveryResult {
    PageFaultDeliveryResult::Delivered {
        target_rip,
        new_rsp,
    }
}

/// Why [`deliver_page_fault_cpl0_set`] refused to deliver. A refusal happens before any
/// memory write: the CPU state and the guest memory are as they were (only the IDT and GDT
/// reads preceded it). Every refusal is a stop of the caller, never a substitute delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0PageFaultRefusal {
    NotPageFault,
    MissingFaultMetadata,
    FaultRipMismatch,
    NotRing0Long64,
    ActivityOutsideProfile,
    StiShadowActive,
    MissingIdtr,
    MissingGdtr,
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    /// The gate of vector 14 (offset 224..=239) lies beyond the IDT limit. The CPU would
    /// raise `#GP` with the IDT selector error code (14 * 8 + 2); this is not delivered.
    GateBeyondIdtLimit,
    NonCanonicalIdtSpan,
    /// Gate P = 0 (the CPU would raise `#NP`); not delivered.
    GateNotPresent,
    /// The gate is not an interrupt gate (0xE) or a trap gate (0xF), or its S bit is set.
    /// Carries the raw type/attribute byte.
    InvalidGateType(u8),
    /// The gate asks for an interrupt stack table entry (IST 1..=7); not modeled.
    UnsupportedIst(u8),
    /// Reserved bits of the gate (byte 4 bits 3..7, or the reserved dword) are not zero.
    GateReservedBitsSet,
    InvalidTargetSelector,
    TargetBeyondGdtLimit,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
    /// S4-39d, a delivery from CPL3 only: no task register is modeled (no `LTR`), so the CPL0
    /// stack (`TSS.RSP0`) is unknown.
    MissingTaskRegister,
    /// S4-39d, a delivery from CPL3 only: `TSS.RSP0` (bytes 4..=11 of the TSS) lies beyond the
    /// task register limit.
    TssRsp0BeyondLimit,
    /// S4-39d, a delivery from CPL3 only: the span of `TSS.RSP0` or the value read from it is
    /// not canonical.
    NonCanonicalRsp0,
}

/// Where [`deliver_page_fault_cpl0_set`] failed on a memory access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0PageFaultDeliveryPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    /// S4-39d, a delivery from CPL3 only: the read of `TSS.RSP0`.
    TssRsp0Read,
    StackWrite {
        slot: PageFaultFrameSlot,
    },
    TargetCodeDescriptorAccessedWrite,
}

/// Result of [`deliver_page_fault_cpl0_set`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cpl0PageFaultDelivery {
    /// The frame is on the stack and CS, RIP, RSP, RFLAGS and CR2 are committed. Nothing was
    /// retired. `trap_gate` is true for gate type 0xF (IF is left as it was).
    Delivered {
        target_rip: u64,
        new_rsp: u64,
        trap_gate: bool,
    },
    /// Nothing was written and the CPU state is unchanged.
    Refused(Cpl0PageFaultRefusal),
    /// A memory access of the delivery failed (a `#PF` here would be a double fault, which is
    /// not modeled; the caller stops). The CPU state is unchanged, but frame words already
    /// stored stay in memory below the stack selected for this delivery (the old RSP for CPL0,
    /// `TSS.RSP0` for CPL3); that partial frame is dead. With the frame inside one page the first
    /// store decides, so nothing is stored then.
    Aborted {
        phase: Cpl0PageFaultDeliveryPhase,
        cause: MemoryError,
    },
}

/// RFLAGS bits the gate entry clears in the live flags: TF, NT, RF and VM (and IF for an
/// interrupt gate). AC is NOT cleared by the entry (from memory of the SDM; indirectly, Linux
/// executes CLAC in its entry code, which would be redundant if the entry cleared AC).
const GATE_CLEARED_FLAGS: u64 = (1 << 8) | (1 << 14) | (1 << 16) | (1 << 17);

/// Refusal reasons shared by the IDT gate delivery of the CPL0 set (a `#PF` and an external
/// interrupt). A refusal happens before any memory write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GateRefusal {
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    MissingIdtr,
    MissingGdtr,
    GateBeyondIdtLimit,
    NonCanonicalIdtSpan,
    GateNotPresent,
    InvalidGateType(u8),
    UnsupportedIst(u8),
    GateReservedBitsSet,
    InvalidTargetSelector,
    TargetBeyondGdtLimit,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
}

/// Where a gate delivery failed on a memory access; `StackWrite` carries the zero-based
/// index of the frame word in store order (0 = SS, 1 = RSP, 2 = RFLAGS, 3 = CS, 4 = RIP,
/// 5 = error code). The tests count the stores from one (store 1 = SS = index 0, store 5 = RIP,
/// store 6 = the error code or, without one, the Accessed-bit write).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GateAbortPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    StackWrite(usize),
    TargetCodeDescriptorAccessedWrite,
}

enum GateFailure {
    Refused(GateRefusal),
    Aborted {
        phase: GateAbortPhase,
        cause: MemoryError,
    },
}

use GateFailure::{Aborted as GateAborted, Refused as GateRefused};

/// A gate and target code descriptor that passed every read-only check.
struct ResolvedGate {
    old_cs: u16,
    old_ss: u16,
    target_selector: u16,
    target_rip: u64,
    descriptor: [u8; 8],
    descriptor_address: u64,
    trap_gate: bool,
    /// The gate DPL (attribute byte bits 5..=6), checked only by the software `INT n` family.
    dpl: u8,
    /// S4-39d: `Some(TSS.RSP0)` for a delivery from CPL3 (the new stack and the CPL0 privilege of
    /// the frame stores), `None` for a CPL0 to CPL0 delivery. Set by the wrapper after
    /// [`read_tss_rsp0`], before the commit.
    kernel_rsp: Option<u64>,
}

/// S4-39d: why the read of `TSS.RSP0` failed; the pair of [`GateFailure`] for the one step only a
/// delivery from CPL3 makes. It is read-only: nothing was written.
enum TssFailure {
    MissingTaskRegister,
    TssRsp0BeyondLimit,
    NonCanonicalRsp0,
    Aborted(MemoryError),
}

/// S4-39d: `RSP0` of the 64-bit TSS at `TR.base + 4`, read as a descriptor-table access (a CPL0
/// page privilege, also under PTI where the TSS is a supervisor page of the user page tables).
/// The read is the last check before the first write of a delivery from CPL3.
fn read_tss_rsp0(
    state: &LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
) -> Result<u64, TssFailure> {
    let Some(tr) = state.tr else {
        return Err(TssFailure::MissingTaskRegister);
    };
    // RSP0 occupies bytes 4..=11 of the TSS.
    if tr.limit < 11 {
        return Err(TssFailure::TssRsp0BeyondLimit);
    }
    let (Some(address), Some(last)) = (tr.base.checked_add(4), tr.base.checked_add(11)) else {
        return Err(TssFailure::NonCanonicalRsp0);
    };
    if !canonical(address) || !canonical(last) {
        return Err(TssFailure::NonCanonicalRsp0);
    }
    let rsp0 = memory
        .read_integer(
            address,
            context.descriptor_table(),
            crate::OperandSize::Qword,
        )
        .map_err(TssFailure::Aborted)?;
    if !canonical(rsp0) {
        return Err(TssFailure::NonCanonicalRsp0);
    }
    Ok(rsp0)
}

/// Read and check the gate of `vector` and its target code descriptor: no write, no change of
/// the CPU state. The wrapper has already checked the entry conditions of its own event.
///
/// `from_cpl3` (S4-39d, only the `#PF` and `#GP` wrappers pass it) asks for a CPL3 state in place
/// of a CPL0 one: CS RPL 3 over a 64-bit code cache with DPL 3, SS selector and cache loaded.
fn resolve_cpl0_gate(
    state: &LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    vector: u8,
    from_cpl3: bool,
) -> Result<ResolvedGate, GateFailure> {
    use GateRefusal as Refusal;
    let refuse = GateRefused;
    let (Some(old_cs), Some(old_cs_cache), Some(old_ss)) =
        (state.cs_selector, state.cs_cache, state.ss_selector)
    else {
        return Err(refuse(Refusal::MissingCurrentSelectors));
    };
    if from_cpl3 {
        if state.ss_cache.is_none() {
            return Err(refuse(Refusal::MissingCurrentSelectors));
        }
        if !valid_ring3_long_code(old_cs, old_cs_cache.access, old_cs_cache.flags)
            || old_ss & 3 != 3
        {
            return Err(refuse(Refusal::InvalidCurrentCodeSegment));
        }
    } else if !valid_ring0_long_code(old_cs, old_cs_cache.access, old_cs_cache.flags) {
        return Err(refuse(Refusal::InvalidCurrentCodeSegment));
    }
    let Some(idtr) = state.idtr else {
        return Err(refuse(Refusal::MissingIdtr));
    };
    let Some(gdtr) = state.gdtr else {
        return Err(refuse(Refusal::MissingGdtr));
    };

    // The gate: 16 bytes at idt_base + 16 * vector, read as two qwords.
    let gate_offset = u64::from(vector) * 16;
    if gate_offset + 15 > u64::from(idtr.limit) {
        return Err(refuse(Refusal::GateBeyondIdtLimit));
    }
    let (Some(gate_address), Some(gate_last)) = (
        idtr.base.checked_add(gate_offset),
        idtr.base.checked_add(gate_offset + 15),
    ) else {
        return Err(refuse(Refusal::NonCanonicalIdtSpan));
    };
    if !canonical(gate_address) || !canonical(gate_last) {
        return Err(refuse(Refusal::NonCanonicalIdtSpan));
    }
    let table = context.descriptor_table();
    let mut gate = [0_u8; 16];
    for half in 0..2_u64 {
        match memory.read_integer(gate_address + 8 * half, table, crate::OperandSize::Qword) {
            Ok(value) => {
                gate[8 * half as usize..8 * half as usize + 8].copy_from_slice(&value.to_le_bytes())
            }
            Err(cause) => {
                return Err(GateAborted {
                    phase: GateAbortPhase::IdtGateRead,
                    cause,
                });
            }
        }
    }
    let attributes = gate[5];
    if attributes & 0x80 == 0 {
        return Err(refuse(Refusal::GateNotPresent));
    }
    let gate_type = attributes & 0x0f;
    if attributes & 0x10 != 0 || (gate_type != 0x0e && gate_type != 0x0f) {
        return Err(refuse(Refusal::InvalidGateType(attributes)));
    }
    if gate[4] & 7 != 0 {
        return Err(refuse(Refusal::UnsupportedIst(gate[4] & 7)));
    }
    if gate[4] & 0xf8 != 0 || gate[12..16] != [0, 0, 0, 0] {
        return Err(refuse(Refusal::GateReservedBitsSet));
    }
    let trap_gate = gate_type == 0x0f;
    let target_selector = u16::from_le_bytes([gate[2], gate[3]]);
    if target_selector & 0xfffc == 0 || target_selector & 4 != 0 || target_selector & 3 != 0 {
        return Err(refuse(Refusal::InvalidTargetSelector));
    }
    let target_rip = u64::from(u16::from_le_bytes([gate[0], gate[1]]))
        | (u64::from(u16::from_le_bytes([gate[6], gate[7]])) << 16)
        | (u64::from(u32::from_le_bytes([gate[8], gate[9], gate[10], gate[11]])) << 32);
    if !canonical(target_rip) {
        return Err(refuse(Refusal::NonCanonicalTargetRip));
    }

    // The target code segment, from the GDT (never from the cache).
    let descriptor_offset = u64::from(target_selector & !7);
    if descriptor_offset + 7 > u64::from(gdtr.limit) {
        return Err(refuse(Refusal::TargetBeyondGdtLimit));
    }
    let (Some(descriptor_address), Some(descriptor_last)) = (
        gdtr.base.checked_add(descriptor_offset),
        gdtr.base.checked_add(descriptor_offset + 7),
    ) else {
        return Err(refuse(Refusal::NonCanonicalGdtSpan));
    };
    if !canonical(descriptor_address) || !canonical(descriptor_last) {
        return Err(refuse(Refusal::NonCanonicalGdtSpan));
    }
    let descriptor = match memory.read_integer(descriptor_address, table, crate::OperandSize::Qword)
    {
        Ok(value) => value.to_le_bytes(),
        Err(cause) => {
            return Err(GateAborted {
                phase: GateAbortPhase::TargetCodeDescriptorRead,
                cause,
            });
        }
    };
    let access = descriptor[5];
    let flags = descriptor[6] >> 4;
    // The accessed bit is set at the commit if clear, so it is not a validity condition.
    if !valid_ring0_long_code(target_selector, access | 1, flags) {
        return Err(refuse(Refusal::InvalidTargetCodeDescriptor));
    }
    Ok(ResolvedGate {
        old_cs,
        old_ss,
        target_selector,
        target_rip,
        descriptor,
        descriptor_address,
        trap_gate,
        dpl: (attributes >> 5) & 3,
        kernel_rsp: None,
    })
}

/// Store the frame, set the target descriptor Accessed bit if clear, and commit CS, RIP, RSP
/// and the live flags. The frame is SS, old RSP, `saved_rflags`, old CS, `saved_rip` and, if
/// present, `error_code` (stored in this order downwards from the 16-byte aligned RSP). A
/// failure leaves the CPU state unchanged. Returns `(target_rip, new_rsp)`.
///
/// S4-39d: with `gate.kernel_rsp = Some(rsp0)` (a delivery from CPL3) the frame is stored
/// downwards from `rsp0` aligned down to 16 in place of the old RSP, with the CPL0 privilege of the
/// new level for the page checks of the stores; the old SS:RSP of the frame are the user ones. The
/// commit then also loads SS with the null selector (64-bit mode, privilege change): selector 0, the
/// cache kept and marked unusable (the S2-46 convention). The rest is the CPL0 to CPL0 delivery.
fn commit_cpl0_gate(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    gate: &ResolvedGate,
    saved_rflags: u64,
    saved_rip: u64,
    error_code: Option<u64>,
) -> Result<(u64, u64), GateFailure> {
    let table = context.descriptor_table();
    let old_rsp = state.gpr[crate::Register::Rsp as usize];
    let aligned_rsp = gate.kernel_rsp.unwrap_or(old_rsp) & !15;
    let words = 5 + u64::from(error_code.is_some());
    let Some(new_rsp) = aligned_rsp.checked_sub(8 * words) else {
        return Err(GateRefused(GateRefusal::StackAddressOverflow));
    };
    if !canonical(new_rsp) || !canonical(aligned_rsp - 1) {
        return Err(GateRefused(GateRefusal::NonCanonicalStackSpan));
    }
    let frame = [
        u64::from(gate.old_ss),
        old_rsp,
        saved_rflags,
        u64::from(gate.old_cs),
        saved_rip,
        error_code.unwrap_or(0),
    ];
    // The frame is stored at the new privilege level: CPL0 after a delivery from CPL3.
    let stack_context = AccessContext {
        origin: AccessOrigin::ImplicitStack,
        privilege: if gate.kernel_rsp.is_some() {
            PrivilegeLevel::Ring0
        } else {
            context.privilege
        },
        ..context
    };
    for (index, value) in frame.iter().take(words as usize).enumerate() {
        let address = aligned_rsp - 8 * (index as u64 + 1);
        if let Err(cause) = memory.write(address, stack_context, &value.to_le_bytes()) {
            return Err(GateAborted {
                phase: GateAbortPhase::StackWrite(index),
                cause,
            });
        }
    }
    let access = gate.descriptor[5];
    let flags = gate.descriptor[6] >> 4;
    if access & 1 == 0 {
        let mut accessed = gate.descriptor;
        accessed[5] |= 1;
        if let Err(cause) = memory.write(gate.descriptor_address, table, &accessed) {
            return Err(GateAborted {
                phase: GateAbortPhase::TargetCodeDescriptorAccessedWrite,
                cause,
            });
        }
    }

    // Commit. SS (selector, cache, unusable mark) stays as it was, unless a CPL3 delivery nulls it.
    state.cs_selector = Some(gate.target_selector);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: access | 1,
        flags,
    });
    state.gpr[crate::Register::Rsp as usize] = new_rsp;
    if gate.kernel_rsp.is_some() {
        state.ss_selector = Some(0);
        state.ss_unusable = true;
    }
    state.rip = gate.target_rip;
    let mut cleared = GATE_CLEARED_FLAGS;
    if !gate.trap_gate {
        cleared |= 1 << 9;
    }
    state.rflags = (state.rflags & !cleared) | (1 << 1);
    Ok((gate.target_rip, new_rsp))
}

macro_rules! impl_from_gate_refusal {
    ($target:ident) => {
        impl From<GateRefusal> for $target {
            fn from(refusal: GateRefusal) -> Self {
                match refusal {
                    GateRefusal::MissingCurrentSelectors => Self::MissingCurrentSelectors,
                    GateRefusal::InvalidCurrentCodeSegment => Self::InvalidCurrentCodeSegment,
                    GateRefusal::MissingIdtr => Self::MissingIdtr,
                    GateRefusal::MissingGdtr => Self::MissingGdtr,
                    GateRefusal::GateBeyondIdtLimit => Self::GateBeyondIdtLimit,
                    GateRefusal::NonCanonicalIdtSpan => Self::NonCanonicalIdtSpan,
                    GateRefusal::GateNotPresent => Self::GateNotPresent,
                    GateRefusal::InvalidGateType(attributes) => Self::InvalidGateType(attributes),
                    GateRefusal::UnsupportedIst(ist) => Self::UnsupportedIst(ist),
                    GateRefusal::GateReservedBitsSet => Self::GateReservedBitsSet,
                    GateRefusal::InvalidTargetSelector => Self::InvalidTargetSelector,
                    GateRefusal::TargetBeyondGdtLimit => Self::TargetBeyondGdtLimit,
                    GateRefusal::NonCanonicalGdtSpan => Self::NonCanonicalGdtSpan,
                    GateRefusal::InvalidTargetCodeDescriptor => Self::InvalidTargetCodeDescriptor,
                    GateRefusal::NonCanonicalTargetRip => Self::NonCanonicalTargetRip,
                    GateRefusal::StackAddressOverflow => Self::StackAddressOverflow,
                    GateRefusal::NonCanonicalStackSpan => Self::NonCanonicalStackSpan,
                }
            }
        }
    };
}
impl_from_gate_refusal!(Cpl0PageFaultRefusal);
impl_from_gate_refusal!(Cpl0ExternalInterruptRefusal);
impl_from_gate_refusal!(Cpl0Int3Refusal);
impl_from_gate_refusal!(Cpl0GeneralProtectionRefusal);

/// Deliver a synchronous page fault of the Long64 CPL0 profile set through the IDT gate of
/// vector 14 (S4-38L5c): CPL0 to CPL0, interrupt gate (type 0xE) or trap gate (0xF), IST 0.
///
/// S4-39d: a `Ring3` context over a CPL3 state (CS RPL 3, DPL 3 64-bit code cache, SS loaded) is
/// delivered too, through the same gate checks and the CPL0 stack of `TSS.RSP0` (TR base + 4,
/// canonical; no task register or a limit below 11 or a non-canonical value refuse before any
/// write). The frame is the same, stored at CPL0 from `RSP0` aligned down to 16, with the user SS,
/// RSP, RFLAGS, CS and RIP; the entry loads CS from the gate and SS with the null selector.
/// Other state/context pairs are `NotRing0Long64`.
///
/// The caller runs the faulting instruction first; it did not retire and RIP is still on it
/// (`record.saved_rip() == state.rip`). Everything is checked before the first write; the
/// frame is then stored, the target code descriptor Accessed bit is set if clear, and the CPU
/// state is committed last (so a failed delivery leaves it unchanged).
///
/// Frame (IA-32e, always with SS and RSP, also without a privilege change): RSP is aligned
/// down to 16, then eight-byte words are stored in this order downwards: SS, old RSP, RFLAGS,
/// CS, saved RIP, error code. RFLAGS in the frame is the live RFLAGS with RF = 1 (a fault) and
/// bit 1 set. After the entry: CS = the gate selector (cache base 0, limit 4 GiB - 1, access
/// with A = 1, flags of the descriptor), RIP = the gate offset, RSP = the frame, SS unchanged
/// (no privilege change loads no new SS), live RFLAGS with TF, NT, RF and VM cleared (AC is kept) and IF
/// cleared for an interrupt gate, CR2 = the faulting linear address.
///
/// SDM (Vol. 3, 6.12 and 6.14), from memory, the text is not in the repository: the frame
/// layout and the 16-byte alignment (6.14.2); the always-pushed SS:RSP (6.14.2); RF = 1 in the
/// frame of a fault (Vol. 3, resume flag section); TF/NT/RF/VM cleared and IF cleared for an
/// interrupt gate (6.12.1.3, also the IA-32e pseudocode of INT n). That AC is not cleared by
/// the entry, and that SS is not reloaded without a CPL change, is also from memory. The gate DPL is not
/// checked (a hardware exception). The values of undefined arithmetic flags are copied into
/// the frame as the model holds them.
pub fn deliver_page_fault_cpl0_set(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    record: &crate::memory::PageFaultRecord,
    context: AccessContext,
) -> Cpl0PageFaultDelivery {
    use Cpl0PageFaultDelivery::Refused;
    use Cpl0PageFaultRefusal as Refusal;
    let refuse = Refused;
    let fault = record.fault();
    if fault.vector != 14 {
        return refuse(Refusal::NotPageFault);
    }
    let (Some(error_code), Some(linear_address)) = (fault.error_code, fault.linear_address) else {
        return refuse(Refusal::MissingFaultMetadata);
    };
    if state.rip != record.saved_rip() {
        return refuse(Refusal::FaultRipMismatch);
    }
    let Some(from_cpl3) = delivery_source(state, context) else {
        return refuse(Refusal::NotRing0Long64);
    };
    if state.activity != Some(crate::CpuActivity::Running) {
        return refuse(Refusal::ActivityOutsideProfile);
    }
    if state.sti_shadow != Some(crate::StiShadow::Clear) {
        return refuse(Refusal::StiShadowActive);
    }
    let mut gate = match resolve_cpl0_gate(state, memory, context, 14, from_cpl3) {
        Ok(gate) => gate,
        Err(failure) => return pf_gate_failure(failure),
    };
    if from_cpl3 {
        match read_tss_rsp0(state, memory, context) {
            Ok(rsp0) => gate.kernel_rsp = Some(rsp0),
            Err(failure) => return pf_tss_failure(failure),
        }
    }
    let saved_rflags = state.rflags | (1 << 16) | (1 << 1);
    match commit_cpl0_gate(
        state,
        memory,
        context,
        &gate,
        saved_rflags,
        record.saved_rip(),
        Some(u64::from(error_code)),
    ) {
        Ok((target_rip, new_rsp)) => {
            state.cr2 = Some(linear_address);
            Cpl0PageFaultDelivery::Delivered {
                target_rip,
                new_rsp,
                trap_gate: gate.trap_gate,
            }
        }
        Err(failure) => pf_gate_failure(failure),
    }
}

fn pf_gate_failure(failure: GateFailure) -> Cpl0PageFaultDelivery {
    match failure {
        GateRefused(refusal) => Cpl0PageFaultDelivery::Refused(refusal.into()),
        GateAborted { phase, cause } => {
            const SLOTS: [PageFaultFrameSlot; 6] = [
                PageFaultFrameSlot::OldSs,
                PageFaultFrameSlot::OldRsp,
                PageFaultFrameSlot::OldRflags,
                PageFaultFrameSlot::OldCs,
                PageFaultFrameSlot::SavedRip,
                PageFaultFrameSlot::ErrorCode,
            ];
            let phase = match phase {
                GateAbortPhase::IdtGateRead => Cpl0PageFaultDeliveryPhase::IdtGateRead,
                GateAbortPhase::TargetCodeDescriptorRead => {
                    Cpl0PageFaultDeliveryPhase::TargetCodeDescriptorRead
                }
                GateAbortPhase::StackWrite(index) => {
                    Cpl0PageFaultDeliveryPhase::StackWrite { slot: SLOTS[index] }
                }
                GateAbortPhase::TargetCodeDescriptorAccessedWrite => {
                    Cpl0PageFaultDeliveryPhase::TargetCodeDescriptorAccessedWrite
                }
            };
            Cpl0PageFaultDelivery::Aborted { phase, cause }
        }
    }
}

/// S4-39d: where an exception delivery of the `#PF`/`#GP` wrappers starts: `Some(false)` at CPL0
/// (Ring0 context), `Some(true)` at CPL3 (Ring3 context over a CS with RPL 3), `None` for any
/// other mode, privilege, or a Ring3 context over a CS that is not RPL 3 (the wrapper refuses
/// with `NotRing0Long64`). Ring1 and Ring2 do not exist in the profile.
fn delivery_source(state: &LongModeState, context: AccessContext) -> Option<bool> {
    if context.mode != InstructionMode::Long64 {
        return None;
    }
    match context.privilege {
        PrivilegeLevel::Ring0 => Some(false),
        PrivilegeLevel::Ring3 if state.cs_selector.is_some_and(|cs| cs & 3 == 3) => Some(true),
        _ => None,
    }
}

fn pf_tss_failure(failure: TssFailure) -> Cpl0PageFaultDelivery {
    match failure {
        TssFailure::MissingTaskRegister => {
            Cpl0PageFaultDelivery::Refused(Cpl0PageFaultRefusal::MissingTaskRegister)
        }
        TssFailure::TssRsp0BeyondLimit => {
            Cpl0PageFaultDelivery::Refused(Cpl0PageFaultRefusal::TssRsp0BeyondLimit)
        }
        TssFailure::NonCanonicalRsp0 => {
            Cpl0PageFaultDelivery::Refused(Cpl0PageFaultRefusal::NonCanonicalRsp0)
        }
        TssFailure::Aborted(cause) => Cpl0PageFaultDelivery::Aborted {
            phase: Cpl0PageFaultDeliveryPhase::TssRsp0Read,
            cause,
        },
    }
}

/// Why [`deliver_general_protection_cpl0_set`] refused to deliver. A refusal happens before any
/// memory write: the CPU state and the guest memory are as they were (only the IDT and GDT
/// reads preceded it). Every refusal is a stop of the caller, never a substitute delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0GeneralProtectionRefusal {
    NotGeneralProtection,
    /// The fault carries no error code (a `#GP` always has one).
    MissingFaultMetadata,
    NotRing0Long64,
    ActivityOutsideProfile,
    StiShadowActive,
    MissingIdtr,
    MissingGdtr,
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    /// The gate of vector 13 (offset 208..=223) lies beyond the IDT limit. The CPU would raise
    /// `#GP` with the IDT selector error code (13 * 8 + 2), which would be a double fault when
    /// raised while delivering a `#GP`; this is not delivered.
    GateBeyondIdtLimit,
    NonCanonicalIdtSpan,
    /// Gate P = 0 (the CPU would raise `#NP`); not delivered.
    GateNotPresent,
    /// The gate is not an interrupt gate (0xE) or a trap gate (0xF), or its S bit is set.
    /// Carries the raw type/attribute byte.
    InvalidGateType(u8),
    /// The gate asks for an interrupt stack table entry (IST 1..=7); not modeled.
    UnsupportedIst(u8),
    GateReservedBitsSet,
    InvalidTargetSelector,
    TargetBeyondGdtLimit,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
    /// S4-39d, a delivery from CPL3 only: no task register is modeled (no `LTR`), so the CPL0
    /// stack (`TSS.RSP0`) is unknown.
    MissingTaskRegister,
    /// S4-39d, a delivery from CPL3 only: `TSS.RSP0` (bytes 4..=11 of the TSS) lies beyond the
    /// task register limit.
    TssRsp0BeyondLimit,
    /// S4-39d, a delivery from CPL3 only: the span of `TSS.RSP0` or the value read from it is
    /// not canonical.
    NonCanonicalRsp0,
}

/// Where [`deliver_general_protection_cpl0_set`] failed on a memory access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0GeneralProtectionPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    /// S4-39d, a delivery from CPL3 only: the read of `TSS.RSP0`.
    TssRsp0Read,
    StackWrite {
        slot: PageFaultFrameSlot,
    },
    TargetCodeDescriptorAccessedWrite,
}

/// Result of [`deliver_general_protection_cpl0_set`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cpl0GeneralProtectionDelivery {
    /// The frame is on the stack and CS, RIP, RSP and RFLAGS are committed. Nothing was
    /// retired. `trap_gate` is true for gate type 0xF (IF is left as it was).
    Delivered {
        target_rip: u64,
        new_rsp: u64,
        trap_gate: bool,
    },
    /// Nothing was written and the CPU state is unchanged.
    Refused(Cpl0GeneralProtectionRefusal),
    /// A memory access of the delivery failed (a fault here would be a double fault, which is
    /// not modeled; the caller stops). The CPU state is unchanged, but frame words already
    /// stored stay in memory below the stack selected for this delivery (the old RSP for CPL0,
    /// `TSS.RSP0` for CPL3); that partial frame is dead. With the frame inside one page the first
    /// store decides, so nothing is stored then.
    Aborted {
        phase: Cpl0GeneralProtectionPhase,
        cause: MemoryError,
    },
}

/// Deliver a synchronous general-protection fault `#GP` (vector 13, `fault.error_code` =
/// `Some(code)`) of the Long64 CPL0 profile set through the IDT gate of vector 13 (S4-38L9v):
/// CPL0 to CPL0, interrupt gate (type 0xE) or trap gate (0xF), IST 0. The frame, the entry and
/// the refusals are those of [`deliver_page_fault_cpl0_set`], with two differences: CR2 is not
/// touched, and the metadata is the fault alone (there is no page-fault record).
///
/// S4-39d: a CPL3 delivery works as in [`deliver_page_fault_cpl0_set`] (TSS.RSP0 stack, null SS).
///
/// The caller runs the faulting instruction first; it did not retire and RIP is still on it:
/// `state.rip` is the saved RIP of the frame (the instruction is restarted by a handler that
/// returns without a fixup). Everything is checked before the first write; the frame is then
/// stored, the target code descriptor Accessed bit is set if clear, and the CPU state is
/// committed last (so a failed delivery leaves it unchanged).
///
/// Frame (IA-32e, SS and RSP always): RSP aligned down to 16, then eight-byte words downwards:
/// SS, old RSP, RFLAGS (live, RF = 1 for a fault, bit 1 set), CS, RIP, error code. After the
/// entry: CS = the gate selector, RIP = the gate offset, RSP = the frame, SS unchanged, live
/// RFLAGS with TF, NT, RF and VM cleared (AC kept) and IF cleared for an interrupt gate.
///
/// SDM (Vol. 3, 6.12 and 6.15), from memory, the text is not in the repository: the layout and
/// the error code of `#GP` (the segment selector error code, 0 when not segment related); RF = 1
/// in the frame of a fault; the entry flag rules; the gate DPL is not checked (a hardware
/// exception).
pub fn deliver_general_protection_cpl0_set(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    fault: &crate::memory::ArchitecturalFault,
    context: AccessContext,
) -> Cpl0GeneralProtectionDelivery {
    use Cpl0GeneralProtectionDelivery::Refused;
    use Cpl0GeneralProtectionRefusal as Refusal;
    let refuse = Refused;
    if fault.vector != 13 {
        return refuse(Refusal::NotGeneralProtection);
    }
    let Some(error_code) = fault.error_code else {
        return refuse(Refusal::MissingFaultMetadata);
    };
    let Some(from_cpl3) = delivery_source(state, context) else {
        return refuse(Refusal::NotRing0Long64);
    };
    if state.activity != Some(crate::CpuActivity::Running) {
        return refuse(Refusal::ActivityOutsideProfile);
    }
    if state.sti_shadow != Some(crate::StiShadow::Clear) {
        return refuse(Refusal::StiShadowActive);
    }
    let mut gate = match resolve_cpl0_gate(state, memory, context, 13, from_cpl3) {
        Ok(gate) => gate,
        Err(failure) => return gp_gate_failure(failure),
    };
    if from_cpl3 {
        match read_tss_rsp0(state, memory, context) {
            Ok(rsp0) => gate.kernel_rsp = Some(rsp0),
            Err(failure) => return gp_tss_failure(failure),
        }
    }
    let saved_rflags = state.rflags | (1 << 16) | (1 << 1);
    let saved_rip = state.rip;
    match commit_cpl0_gate(
        state,
        memory,
        context,
        &gate,
        saved_rflags,
        saved_rip,
        Some(u64::from(error_code)),
    ) {
        Ok((target_rip, new_rsp)) => Cpl0GeneralProtectionDelivery::Delivered {
            target_rip,
            new_rsp,
            trap_gate: gate.trap_gate,
        },
        Err(failure) => gp_gate_failure(failure),
    }
}

fn gp_gate_failure(failure: GateFailure) -> Cpl0GeneralProtectionDelivery {
    match failure {
        GateRefused(refusal) => Cpl0GeneralProtectionDelivery::Refused(refusal.into()),
        GateAborted { phase, cause } => {
            const SLOTS: [PageFaultFrameSlot; 6] = [
                PageFaultFrameSlot::OldSs,
                PageFaultFrameSlot::OldRsp,
                PageFaultFrameSlot::OldRflags,
                PageFaultFrameSlot::OldCs,
                PageFaultFrameSlot::SavedRip,
                PageFaultFrameSlot::ErrorCode,
            ];
            let phase = match phase {
                GateAbortPhase::IdtGateRead => Cpl0GeneralProtectionPhase::IdtGateRead,
                GateAbortPhase::TargetCodeDescriptorRead => {
                    Cpl0GeneralProtectionPhase::TargetCodeDescriptorRead
                }
                GateAbortPhase::StackWrite(index) => {
                    Cpl0GeneralProtectionPhase::StackWrite { slot: SLOTS[index] }
                }
                GateAbortPhase::TargetCodeDescriptorAccessedWrite => {
                    Cpl0GeneralProtectionPhase::TargetCodeDescriptorAccessedWrite
                }
            };
            Cpl0GeneralProtectionDelivery::Aborted { phase, cause }
        }
    }
}

fn gp_tss_failure(failure: TssFailure) -> Cpl0GeneralProtectionDelivery {
    match failure {
        TssFailure::MissingTaskRegister => Cpl0GeneralProtectionDelivery::Refused(
            Cpl0GeneralProtectionRefusal::MissingTaskRegister,
        ),
        TssFailure::TssRsp0BeyondLimit => {
            Cpl0GeneralProtectionDelivery::Refused(Cpl0GeneralProtectionRefusal::TssRsp0BeyondLimit)
        }
        TssFailure::NonCanonicalRsp0 => {
            Cpl0GeneralProtectionDelivery::Refused(Cpl0GeneralProtectionRefusal::NonCanonicalRsp0)
        }
        TssFailure::Aborted(cause) => Cpl0GeneralProtectionDelivery::Aborted {
            phase: Cpl0GeneralProtectionPhase::TssRsp0Read,
            cause,
        },
    }
}

/// Why [`deliver_external_interrupt_cpl0_set`] refused to deliver. A refusal happens before
/// any memory write: the CPU state and the guest memory are as they were (only the IDT and GDT
/// reads preceded it). It is the caller's refusal to call the function now (for example
/// `InterruptsMasked`), not an architectural fault, and never a substitute delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0ExternalInterruptRefusal {
    NotRing0Long64,
    /// The CPU is not `Running` (halted, shut down, or unknown).
    ActivityOutsideProfile,
    /// RFLAGS.IF = 0: a maskable external interrupt is not accepted.
    InterruptsMasked,
    /// The one-instruction interrupt shadow after `STI` is active (or unknown).
    StiShadowActive,
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    MissingIdtr,
    MissingGdtr,
    /// The gate of the vector (offset `16 * vector ..= 16 * vector + 15`) lies beyond the IDT
    /// limit.
    GateBeyondIdtLimit,
    NonCanonicalIdtSpan,
    /// Gate P = 0.
    GateNotPresent,
    /// The gate is not an interrupt gate (0xE) or a trap gate (0xF), or its S bit is set.
    /// Carries the raw type/attribute byte.
    InvalidGateType(u8),
    /// A trap gate (0xF): it would leave IF set, which an external interrupt does not use;
    /// not modeled.
    TrapGateNotSupported,
    /// The gate asks for an interrupt stack table entry (IST 1..=7); not modeled.
    UnsupportedIst(u8),
    GateReservedBitsSet,
    InvalidTargetSelector,
    TargetBeyondGdtLimit,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
    /// A delivery from CPL3 only: no task register is modeled, so `TSS.RSP0` is unknown.
    MissingTaskRegister,
    /// A delivery from CPL3 only: `TSS.RSP0` lies beyond the task register limit.
    TssRsp0BeyondLimit,
    /// A delivery from CPL3 only: the TSS.RSP0 address or value is not canonical.
    NonCanonicalRsp0,
}

/// Where [`deliver_external_interrupt_cpl0_set`] failed on a memory access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0ExternalInterruptPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    /// A delivery from CPL3 only: the read of `TSS.RSP0`.
    TssRsp0Read,
    StackWrite {
        slot: InterruptFrameSlot,
    },
    TargetCodeDescriptorAccessedWrite,
}

/// Result of [`deliver_external_interrupt_cpl0_set`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cpl0ExternalInterruptDelivery {
    /// The frame is on the stack and CS, RIP, RSP and RFLAGS are committed (IF cleared). Nothing
    /// was retired.
    Delivered { target_rip: u64, new_rsp: u64 },
    /// Nothing was written and the CPU state is unchanged.
    Refused(Cpl0ExternalInterruptRefusal),
    /// A memory access of the delivery failed (a `#PF` here would be a double fault, which is
    /// not modeled; the caller stops). The CPU state is unchanged, but frame words already
    /// stored stay in memory below the stack selected for this delivery (the old RSP for CPL0,
    /// `TSS.RSP0` for CPL3); that partial frame is dead. With the frame inside one page the first
    /// store decides, so nothing is stored then.
    Aborted {
        phase: Cpl0ExternalInterruptPhase,
        cause: MemoryError,
    },
}

/// Deliver an external hardware interrupt `vector` of the Long64 CPL0 profile set through its
/// IDT gate (S4-38L8a/S4-39h): CPL0 to CPL0, or CPL3 through `TSS.RSP0` to CPL0; interrupt gate
/// (type 0xE) only, IST 0. The caller calls it on an instruction boundary (nothing is being
/// executed); `state.rip` is the address of the next instruction to run and is saved in the frame.
///
/// Refused before any write when RFLAGS.IF = 0 (`InterruptsMasked`), the STI shadow is not
/// `Clear`, the CPU is not `Running`, the context/state do not describe CPL0 or CPL3 Long64, a
/// trap gate (0xF), a not-present gate or another gate type, IST != 0, and the same descriptor,
/// selector and stack conditions as [`deliver_page_fault_cpl0_set`] (the gate DPL is not
/// checked: it applies to software `INT n` only). A CPL3 delivery also reads `TSS.RSP0` before
/// any frame write.
///
/// Frame, the same IA-32e layout as the `#PF` one but without an error code: RSP is aligned
/// down to 16 (from `TSS.RSP0` on a CPL3 transition), then eight-byte words are stored downwards:
/// SS, old RSP, RFLAGS, CS, RIP. RFLAGS
/// in the frame is the live RFLAGS with RF cleared and bit 1 set (IF stays 1: IF = 1 is a
/// delivery condition, and `IRETQ` restores it). After the entry: CS = the gate selector, RIP =
/// the gate offset, RSP = the frame (old RSP aligned down minus 40 for CPL0, or `TSS.RSP0`
/// aligned down minus 40 for CPL3), SS unchanged for CPL0 or null with its cache marked unusable
/// for CPL3, live RFLAGS with IF, TF, NT, RF and VM cleared (AC kept). CR2 and the retire counters
/// are not touched.
///
/// Intel SDM, combined Volumes 1, 2A-2D, 3A-3D and 4, revision 090, §§7.14.2 and 7.14.4:
/// eight-byte stack-frame entries, 16-byte alignment before the push sequence, and the TSS stack switch
/// with null SS on a CPL change. RF clearing and external-interrupt gate-DPL behavior follow the
/// interrupt delivery rules; software `INT n` has a separate DPL check.
pub fn deliver_external_interrupt_cpl0_set(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    vector: u8,
    context: AccessContext,
) -> Cpl0ExternalInterruptDelivery {
    use Cpl0ExternalInterruptRefusal as Refusal;
    let refuse = Cpl0ExternalInterruptDelivery::Refused;
    let Some(from_cpl3) = delivery_source(state, context) else {
        return refuse(Refusal::NotRing0Long64);
    };
    if state.activity != Some(crate::CpuActivity::Running) {
        return refuse(Refusal::ActivityOutsideProfile);
    }
    if state.rflags & (1 << 9) == 0 {
        return refuse(Refusal::InterruptsMasked);
    }
    if state.sti_shadow != Some(crate::StiShadow::Clear) {
        return refuse(Refusal::StiShadowActive);
    }
    let mut gate = match resolve_cpl0_gate(state, memory, context, vector, from_cpl3) {
        Ok(gate) => gate,
        Err(failure) => return external_gate_failure(failure),
    };
    if gate.trap_gate {
        return refuse(Refusal::TrapGateNotSupported);
    }
    if from_cpl3 {
        match read_tss_rsp0(state, memory, context) {
            Ok(rsp0) => gate.kernel_rsp = Some(rsp0),
            Err(failure) => return external_tss_failure(failure),
        }
    }
    let saved_rflags = (state.rflags & !(1 << 16)) | (1 << 1);
    let saved_rip = state.rip;
    match commit_cpl0_gate(state, memory, context, &gate, saved_rflags, saved_rip, None) {
        Ok((target_rip, new_rsp)) => Cpl0ExternalInterruptDelivery::Delivered {
            target_rip,
            new_rsp,
        },
        Err(failure) => external_gate_failure(failure),
    }
}

fn external_gate_failure(failure: GateFailure) -> Cpl0ExternalInterruptDelivery {
    match failure {
        GateRefused(refusal) => Cpl0ExternalInterruptDelivery::Refused(refusal.into()),
        GateAborted { phase, cause } => {
            const SLOTS: [InterruptFrameSlot; 5] = [
                InterruptFrameSlot::OldSs,
                InterruptFrameSlot::OldRsp,
                InterruptFrameSlot::OldRflags,
                InterruptFrameSlot::OldCs,
                InterruptFrameSlot::OldRip,
            ];
            let phase = match phase {
                GateAbortPhase::IdtGateRead => Cpl0ExternalInterruptPhase::IdtGateRead,
                GateAbortPhase::TargetCodeDescriptorRead => {
                    Cpl0ExternalInterruptPhase::TargetCodeDescriptorRead
                }
                GateAbortPhase::StackWrite(index) => {
                    Cpl0ExternalInterruptPhase::StackWrite { slot: SLOTS[index] }
                }
                GateAbortPhase::TargetCodeDescriptorAccessedWrite => {
                    Cpl0ExternalInterruptPhase::TargetCodeDescriptorAccessedWrite
                }
            };
            Cpl0ExternalInterruptDelivery::Aborted { phase, cause }
        }
    }
}

fn external_tss_failure(failure: TssFailure) -> Cpl0ExternalInterruptDelivery {
    match failure {
        TssFailure::MissingTaskRegister => Cpl0ExternalInterruptDelivery::Refused(
            Cpl0ExternalInterruptRefusal::MissingTaskRegister,
        ),
        TssFailure::TssRsp0BeyondLimit => {
            Cpl0ExternalInterruptDelivery::Refused(Cpl0ExternalInterruptRefusal::TssRsp0BeyondLimit)
        }
        TssFailure::NonCanonicalRsp0 => {
            Cpl0ExternalInterruptDelivery::Refused(Cpl0ExternalInterruptRefusal::NonCanonicalRsp0)
        }
        TssFailure::Aborted(cause) => Cpl0ExternalInterruptDelivery::Aborted {
            phase: Cpl0ExternalInterruptPhase::TssRsp0Read,
            cause,
        },
    }
}

/// Why [`deliver_int3_cpl0_set`] refused to deliver. A refusal happens before any memory write:
/// the CPU state and the guest memory are as they were (only the IDT and GDT reads preceded
/// it). Every refusal is a stop of the caller, never a substitute delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0Int3Refusal {
    NotRing0Long64,
    /// The CPU is not `Running` (halted, shut down, or unknown).
    ActivityOutsideProfile,
    /// The one-instruction interrupt shadow (`STI` or `MOV SS`) is active (or unknown). The
    /// model does not decide what a software trap does under it; it stops.
    StiShadowActive,
    MissingCurrentSelectors,
    InvalidCurrentCodeSegment,
    MissingIdtr,
    MissingGdtr,
    /// The gate of vector 3 (offset 48..=63) lies beyond the IDT limit. The CPU would raise
    /// `#GP` with the IDT selector error code (3 * 8 + 2); this is not delivered.
    GateBeyondIdtLimit,
    NonCanonicalIdtSpan,
    /// Gate P = 0 (the CPU would raise `#NP`); not delivered.
    GateNotPresent,
    /// The gate is not an interrupt gate (0xE) or a trap gate (0xF), or its S bit is set.
    /// Carries the raw type/attribute byte.
    InvalidGateType(u8),
    /// The gate asks for an interrupt stack table entry (IST 1..=7); not modeled.
    UnsupportedIst(u8),
    GateReservedBitsSet,
    InvalidTargetSelector,
    TargetBeyondGdtLimit,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
    /// The software-interrupt DPL check failed (CPL > gate DPL: the CPU would raise `#GP` with
    /// the IDT selector error code 3 * 8 + 2). Unreachable at CPL0 (every DPL is >= 0); kept
    /// so that the check exists and is tested where the model one day runs another CPL.
    GateDplBelowCpl,
}

/// Where [`deliver_int3_cpl0_set`] failed on a memory access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl0Int3Phase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    StackWrite { slot: InterruptFrameSlot },
    TargetCodeDescriptorAccessedWrite,
}

/// Result of [`deliver_int3_cpl0_set`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cpl0Int3Delivery {
    /// The frame is on the stack and CS, RIP, RSP and RFLAGS are committed. `trap_gate` is true
    /// for gate type 0xF (IF left as it was); for 0xE IF is cleared. The caller retires the
    /// `INT3`.
    Delivered {
        target_rip: u64,
        new_rsp: u64,
        trap_gate: bool,
    },
    /// Nothing was written and the CPU state is unchanged.
    Refused(Cpl0Int3Refusal),
    /// A memory access of the delivery failed (the caller reports it as the failed access of the
    /// `INT3`). The CPU state is unchanged, but frame words already stored stay in memory: they
    /// lie below the old RSP (dead stack). With the frame inside one page the first store
    /// decides, so nothing is stored then.
    Aborted {
        phase: Cpl0Int3Phase,
        cause: MemoryError,
    },
}

/// The software-interrupt gate check of `INT n` / `INT3`: the current privilege level must not
/// be numerically greater than the gate DPL (Intel SDM Vol. 3, 6.12.1.1 and the `INT n`
/// pseudocode, from memory: a hardware interrupt or exception does not make this check).
const fn software_gate_dpl_ok(cpl: u8, gate_dpl: u8) -> bool {
    cpl <= gate_dpl
}

/// Deliver the software breakpoint `#BP` (vector 3) of `INT3` (`CC`) in the Long64 CPL0 profile
/// set through its IDT gate (S4-38L9e): CPL0 to CPL0, interrupt gate (type 0xE, what Linux
/// installs: `SYSG(X86_TRAP_BP, asm_exc_int3)` = `GATE_INTERRUPT`, DPL 3, IST 0, in
/// `arch/x86/kernel/idt.c`) or trap gate (0xF), IST 0. `return_rip` is the address of the NEXT
/// instruction (the `INT3` has length 1): a trap saves the address after the instruction, so
/// the handler returns past it. The caller runs the instruction at a boundary, retires it on
/// `Delivered` and does not otherwise change RIP.
///
/// Refused before any write: not Long64 CPL0, the CPU not `Running`, the STI shadow not `Clear`,
/// a gate that is not present or not type 0xE/0xF, IST != 0, a software DPL violation, and the
/// same descriptor, selector and stack conditions as [`deliver_page_fault_cpl0_set`]. A
/// `#GP`/`#NP` the CPU would raise for these is not delivered (the caller stops).
///
/// Frame (IA-32e, the same five qwords as an external interrupt, no error code): RSP is
/// aligned down to 16, then SS, old RSP, RFLAGS, CS and `return_rip` are stored downwards. RFLAGS
/// in the frame is the live RFLAGS with RF cleared and bit 1 set (a trap does not set RF; IF is
/// saved as it is). After the entry: CS = the gate selector, RIP = the gate offset, RSP = the
/// frame, SS unchanged, live RFLAGS with TF, NT, RF and VM cleared (AC kept) and IF cleared for
/// an interrupt gate only. CR2 is untouched.
///
/// SDM, from memory, not checked against the text: `#BP` is a trap taken after the `INT3` with
/// the saved RIP of the next instruction (Vol. 3, 6.15 exception table); the IF rule of the two
/// gate types (6.12.1.3); the DPL check of the software form (6.12.1.1); the frame layout as for
/// the other IA-32e entries.
pub fn deliver_int3_cpl0_set(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    return_rip: u64,
    context: AccessContext,
) -> Cpl0Int3Delivery {
    use Cpl0Int3Refusal as Refusal;
    let refuse = Cpl0Int3Delivery::Refused;
    if context.mode != InstructionMode::Long64 || context.privilege != PrivilegeLevel::Ring0 {
        return refuse(Refusal::NotRing0Long64);
    }
    if state.activity != Some(crate::CpuActivity::Running) {
        return refuse(Refusal::ActivityOutsideProfile);
    }
    if state.sti_shadow != Some(crate::StiShadow::Clear) {
        return refuse(Refusal::StiShadowActive);
    }
    let gate = match resolve_cpl0_gate(state, memory, context, 3, false) {
        Ok(gate) => gate,
        Err(failure) => return int3_gate_failure(failure),
    };
    // By the `INT n` pseudocode (from memory) the DPL check comes BEFORE the present check, so a
    // DPL violation raises `#GP` before `#NP`. At CPL0 the check cannot fail, so the order is
    // not observable here; when another CPL is ever modeled, `GateNotPresent` (resolved above)
    // must not mask this refusal and the refusal type must be revisited.
    if !software_gate_dpl_ok(0, gate.dpl) {
        return refuse(Refusal::GateDplBelowCpl);
    }
    let saved_rflags = (state.rflags & !(1 << 16)) | (1 << 1);
    match commit_cpl0_gate(
        state,
        memory,
        context,
        &gate,
        saved_rflags,
        return_rip,
        None,
    ) {
        Ok((target_rip, new_rsp)) => Cpl0Int3Delivery::Delivered {
            target_rip,
            new_rsp,
            trap_gate: gate.trap_gate,
        },
        Err(failure) => int3_gate_failure(failure),
    }
}

fn int3_gate_failure(failure: GateFailure) -> Cpl0Int3Delivery {
    match failure {
        GateRefused(refusal) => Cpl0Int3Delivery::Refused(refusal.into()),
        GateAborted { phase, cause } => {
            const SLOTS: [InterruptFrameSlot; 5] = [
                InterruptFrameSlot::OldSs,
                InterruptFrameSlot::OldRsp,
                InterruptFrameSlot::OldRflags,
                InterruptFrameSlot::OldCs,
                InterruptFrameSlot::OldRip,
            ];
            let phase = match phase {
                GateAbortPhase::IdtGateRead => Cpl0Int3Phase::IdtGateRead,
                GateAbortPhase::TargetCodeDescriptorRead => Cpl0Int3Phase::TargetCodeDescriptorRead,
                GateAbortPhase::StackWrite(index) => {
                    Cpl0Int3Phase::StackWrite { slot: SLOTS[index] }
                }
                GateAbortPhase::TargetCodeDescriptorAccessedWrite => {
                    Cpl0Int3Phase::TargetCodeDescriptorAccessedWrite
                }
            };
            Cpl0Int3Delivery::Aborted { phase, cause }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterruptOutsideProfile {
    UnsupportedCr0State,
    UnsupportedCr3State,
    InvalidRflagsDefined(u64),
    NotRing0Long64,
    InterruptsMasked,
    InterruptsInhibited,
    HaltedWithoutClearShadow,
    IncompatibleEntryFlags(u64),
    MissingIdtr,
    MissingVisibleCsOrSs,
    IncompatibleCurrentSelectors,
    MissingIdtEntry,
    NonCanonicalIdtSpan,
    InvalidGate,
    InvalidTargetSelector,
    MissingGdtr,
    MissingTargetDescriptor,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    InvalidCurrentCodeSegment,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterruptDeliveryResult {
    OutsideProfile(InterruptOutsideProfile),
    DeliveryAborted {
        phase: InterruptDeliveryPhase,
        cause: MemoryError,
    },
    Delivered {
        vector: u8,
        target_rip: u64,
        new_rsp: u64,
    },
}

/// Deliver exactly one caller-selected external maskable interrupt.
///
/// The caller certifies a fresh completed-instruction boundary, no pending
/// synchronous/debug/NMI event or inhibition, private synchronous RAM, and
/// CET/VMX/SMM/shadow stacks disabled. No instruction fetch or retirement is
/// performed here. A `DeliveryAborted` means some earlier stack writes may
/// already have reached memory; callers must stop and must not retry.
pub fn deliver_maskable_interrupt(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    vector: u8,
    context: AccessContext,
    rflags_defined: u64,
) -> InterruptDeliveryResult {
    deliver_interrupt(state, memory, vector, context, rflags_defined, None)
}

/// Deliver exactly one caller-selected external maskable interrupt on the
/// CPU-owned paged Long64 CPL0 profile (S4-27h). The body is shared with
/// [`deliver_maskable_interrupt`]; only the control-register precondition and the
/// stack-write origin differ. Preconditions match `step_paged_iretq`: CR0 and
/// CR4 equal the BASE profile, CR3 is below 2^36 and EFER is 0xD00. The frame is
/// written as `ImplicitStack` qwords (the flat profile uses `Explicit`). The
/// caller supplies a `GuestMemory` that walks the guest page tables; it
/// certifies the same boundary and exclusions as the flat entry. No nested
/// delivery is modelled: a fault on any table or stack access is returned as
/// `DeliveryAborted` and the caller must stop.
pub fn deliver_paged_maskable_interrupt(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    vector: u8,
    context: AccessContext,
    rflags_defined: u64,
    efer: u64,
) -> InterruptDeliveryResult {
    deliver_interrupt(state, memory, vector, context, rflags_defined, Some(efer))
}

/// `paged_efer` is `Some` for the paged front and `None` for the flat front.
fn deliver_interrupt(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    vector: u8,
    context: AccessContext,
    rflags_defined: u64,
    paged_efer: Option<u64>,
) -> InterruptDeliveryResult {
    match paged_efer {
        None => {
            if state.cr3.is_some() {
                return outside(InterruptOutsideProfile::UnsupportedCr3State);
            }
            if state.cr0.is_some() || state.cr4.is_some() {
                return outside(InterruptOutsideProfile::UnsupportedCr0State);
            }
        }
        Some(efer) => {
            if state.cr3.is_none_or(|cr3| cr3 & !((1u64 << 36) - 1) != 0) || efer != 0xd00 {
                return outside(InterruptOutsideProfile::UnsupportedCr3State);
            }
            if state.cr0 != Some(crate::execute::Cr0Profile::BASE)
                || state.cr4 != Some(crate::execute::Cr4Profile::BASE)
            {
                return outside(InterruptOutsideProfile::UnsupportedCr0State);
            }
        }
    }
    if state.sti_shadow == Some(crate::StiShadow::Blocking) {
        return outside(InterruptOutsideProfile::InterruptsInhibited);
    }
    if state.activity == Some(crate::CpuActivity::Halted)
        && state.sti_shadow != Some(crate::StiShadow::Clear)
    {
        return outside(InterruptOutsideProfile::HaltedWithoutClearShadow);
    }
    if rflags_defined & !INTEGER_FLAGS != 0 {
        return outside(InterruptOutsideProfile::InvalidRflagsDefined(
            rflags_defined,
        ));
    }
    if context.mode != InstructionMode::Long64 || context.privilege != PrivilegeLevel::Ring0 {
        return outside(InterruptOutsideProfile::NotRing0Long64);
    }
    const IF: u64 = 1 << 9;
    const ENTRY_REJECTED: u64 = (1 << 8) | (1 << 16) | (1 << 17) | (1 << 14);
    if state.rflags & IF == 0 {
        return outside(InterruptOutsideProfile::InterruptsMasked);
    }
    let incompatible = state.rflags & ENTRY_REJECTED;
    if incompatible != 0 {
        return outside(InterruptOutsideProfile::IncompatibleEntryFlags(
            incompatible,
        ));
    }
    let (Some(old_cs), Some(old_cs_cache), Some(old_ss), Some(idtr)) = (
        state.cs_selector,
        state.cs_cache,
        state.ss_selector,
        state.idtr,
    ) else {
        return outside(if state.idtr.is_none() {
            InterruptOutsideProfile::MissingIdtr
        } else {
            InterruptOutsideProfile::MissingVisibleCsOrSs
        });
    };
    let old_cs_access = old_cs_cache.access;
    let old_cs_flags = old_cs_cache.flags;
    if old_cs & 0xfffc == 0
        || old_cs & 3 != 0
        || old_ss & 3 != 0
        || old_cs_cache.base != 0
        || old_cs_cache.limit != u32::MAX
        || old_cs_access & 0x18 != 0x18
        || old_cs_access & 0x04 != 0
        || old_cs_access & 0x60 != 0
        || old_cs_access & 0x80 == 0
        || old_cs_flags & 0x2 == 0
        || old_cs_flags & 0x4 != 0
    {
        return outside(InterruptOutsideProfile::IncompatibleCurrentSelectors);
    }

    let idt_offset = u64::from(vector) * 16;
    if idt_offset + 15 > u64::from(idtr.limit) {
        return outside(InterruptOutsideProfile::MissingIdtEntry);
    }
    let Some(gate_address) = idtr.base.checked_add(idt_offset) else {
        return outside(InterruptOutsideProfile::NonCanonicalIdtSpan);
    };
    let Some(gate_last) = gate_address.checked_add(15) else {
        return outside(InterruptOutsideProfile::NonCanonicalIdtSpan);
    };
    if !canonical(gate_address) || !canonical(gate_last) {
        return outside(InterruptOutsideProfile::NonCanonicalIdtSpan);
    }
    let mut gate = [0; 16];
    if let Err(cause) = memory.read(gate_address, context.descriptor_table(), &mut gate) {
        return aborted(InterruptDeliveryPhase::IdtGateRead, cause);
    }
    let gate_type = gate[5] & 0x0f;
    if gate[4] & 0xf8 != 0
        || gate[4] & 7 != 0
        || gate[12..16] != [0, 0, 0, 0]
        || gate_type != 0x0e
        || gate[5] & 0x10 != 0
        || gate[5] & 0x80 == 0
    {
        return outside(InterruptOutsideProfile::InvalidGate);
    }
    let target_selector = u16::from_le_bytes([gate[2], gate[3]]);
    if target_selector & 0xfffc == 0 || target_selector & 4 != 0 {
        return outside(InterruptOutsideProfile::InvalidTargetSelector);
    }
    let Some(gdtr) = state.gdtr else {
        return outside(InterruptOutsideProfile::MissingGdtr);
    };
    let descriptor_offset = u64::from(target_selector & !7);
    if descriptor_offset + 7 > u64::from(gdtr.limit) {
        return outside(InterruptOutsideProfile::MissingTargetDescriptor);
    }
    let Some(descriptor_address) = gdtr.base.checked_add(descriptor_offset) else {
        return outside(InterruptOutsideProfile::NonCanonicalGdtSpan);
    };
    let Some(descriptor_last) = descriptor_address.checked_add(7) else {
        return outside(InterruptOutsideProfile::NonCanonicalGdtSpan);
    };
    if !canonical(descriptor_address) || !canonical(descriptor_last) {
        return outside(InterruptOutsideProfile::NonCanonicalGdtSpan);
    }
    let mut descriptor = [0; 8];
    if let Err(cause) = memory.read(
        descriptor_address,
        context.descriptor_table(),
        &mut descriptor,
    ) {
        return aborted(InterruptDeliveryPhase::TargetCodeDescriptorRead, cause);
    }
    let access = descriptor[5];
    let flags = descriptor[6] >> 4;
    let target_rip = u64::from(u16::from_le_bytes([gate[0], gate[1]]))
        | (u64::from(u16::from_le_bytes([gate[6], gate[7]])) << 16)
        | (u64::from(u32::from_le_bytes(gate[8..12].try_into().unwrap())) << 32);
    if access & 0x10 == 0
        || access & 0x08 == 0
        || access & 0x04 != 0
        || access & 0x60 != 0
        || access & 0x80 == 0
        || access & 1 == 0
        || flags & 0x2 == 0
        || flags & 0x4 != 0
        || !canonical(target_rip)
    {
        return outside(if !canonical(target_rip) {
            InterruptOutsideProfile::NonCanonicalTargetRip
        } else {
            InterruptOutsideProfile::InvalidTargetCodeDescriptor
        });
    }

    let aligned_rsp = state.gpr[crate::Register::Rsp as usize] & !15;
    let Some(new_rsp) = aligned_rsp.checked_sub(40) else {
        return outside(InterruptOutsideProfile::StackAddressOverflow);
    };
    let slots = [
        (aligned_rsp - 8, u64::from(old_ss)),
        (aligned_rsp - 16, state.gpr[crate::Register::Rsp as usize]),
        (aligned_rsp - 24, state.rflags),
        (aligned_rsp - 32, u64::from(old_cs)),
        (aligned_rsp - 40, state.rip),
    ];
    if slots.iter().any(|(address, _)| {
        address
            .checked_add(7)
            .is_none_or(|last| !canonical(*address) || !canonical(last))
    }) {
        return outside(InterruptOutsideProfile::NonCanonicalStackSpan);
    }
    let write_context = AccessContext {
        origin: if paged_efer.is_some() {
            AccessOrigin::ImplicitStack
        } else {
            AccessOrigin::Explicit
        },
        ..context
    };
    let frame_slots = [
        InterruptFrameSlot::OldSs,
        InterruptFrameSlot::OldRsp,
        InterruptFrameSlot::OldRflags,
        InterruptFrameSlot::OldCs,
        InterruptFrameSlot::OldRip,
    ];
    for ((address, value), slot) in slots.into_iter().zip(frame_slots) {
        if let Err(cause) = memory.write(address, write_context, &value.to_le_bytes()) {
            return aborted(InterruptDeliveryPhase::StackWrite { slot }, cause);
        }
    }

    state.cs_selector = Some(target_selector & !3);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access,
        flags,
    });
    state.gpr[crate::Register::Rsp as usize] = new_rsp;
    state.rip = target_rip;
    state.rflags &= !((1 << 9) | (1 << 8) | (1 << 17) | (1 << 16) | (1 << 14));
    if state.activity == Some(crate::CpuActivity::Halted) {
        state.activity = Some(crate::CpuActivity::Running);
    }
    InterruptDeliveryResult::Delivered {
        vector,
        target_rip,
        new_rsp,
    }
}

fn outside(reason: InterruptOutsideProfile) -> InterruptDeliveryResult {
    InterruptDeliveryResult::OutsideProfile(reason)
}

fn aborted(phase: InterruptDeliveryPhase, cause: MemoryError) -> InterruptDeliveryResult {
    InterruptDeliveryResult::DeliveryAborted { phase, cause }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OperandSize;
    use crate::memory::{AccessKind, ArchitecturalFault};

    #[test]
    fn the_software_gate_dpl_check_needs_cpl_not_above_the_gate_dpl() {
        // Intel SDM Vol. 3, 6.12.1.1 (from memory): INT n / INT3 fault when CPL > gate DPL.
        for cpl in 0..=3u8 {
            for dpl in 0..=3u8 {
                assert_eq!(software_gate_dpl_ok(cpl, dpl), cpl <= dpl, "{cpl} {dpl}");
            }
        }
        // CPL0 passes every gate DPL; CPL3 passes only a DPL 3 gate.
        assert!((0..=3).all(|dpl| software_gate_dpl_ok(0, dpl)));
        assert!(software_gate_dpl_ok(3, 3));
        assert!(!software_gate_dpl_ok(3, 0));
        assert!(!software_gate_dpl_ok(1, 0));
    }

    struct Ram {
        bytes: Vec<u8>,
        reads: Vec<(u64, usize, AccessContext)>,
        writes: Vec<(u64, usize, AccessContext)>,
        fetches: usize,
        fail_read: Option<usize>,
        fail_write: Option<usize>,
    }

    impl Ram {
        fn new() -> Self {
            Self {
                bytes: vec![0; 0x10_000],
                reads: Vec::new(),
                writes: Vec::new(),
                fetches: 0,
                fail_read: None,
                fail_write: None,
            }
        }

        fn range(&self, address: u64, len: usize) -> Result<std::ops::Range<usize>, MemoryError> {
            let start = usize::try_from(address).unwrap_or(usize::MAX);
            let end = start.checked_add(len).unwrap_or(usize::MAX);
            if end > self.bytes.len() {
                return Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                });
            }
            Ok(start..end)
        }

        fn configure(&mut self, vector: u8, target: u64, access: u8, flags: u8) {
            let gate_at = 0x2000 + usize::from(vector) * 16;
            self.bytes[gate_at..gate_at + 2].copy_from_slice(&(target as u16).to_le_bytes());
            self.bytes[gate_at + 2..gate_at + 4].copy_from_slice(&0x23u16.to_le_bytes());
            self.bytes[gate_at + 4] = 0;
            self.bytes[gate_at + 5] = 0x8e;
            self.bytes[gate_at + 6..gate_at + 8]
                .copy_from_slice(&((target >> 16) as u16).to_le_bytes());
            self.bytes[gate_at + 8..gate_at + 12]
                .copy_from_slice(&((target >> 32) as u32).to_le_bytes());
            self.bytes[gate_at + 12..gate_at + 16].fill(0);
            let desc_at = 0x1000 + 0x20;
            self.bytes[desc_at..desc_at + 8].fill(0);
            self.bytes[desc_at + 5] = access;
            self.bytes[desc_at + 6] = flags << 4;
        }
    }

    impl GuestMemory for Ram {
        fn preflight_descriptor_accessed_write(
            &mut self,
            descriptor: u64,
            _: AccessContext,
            stack_writes: &[(u64, usize)],
        ) -> Result<(), MemoryError> {
            // This fixture is flat RAM, without translation metadata or aliases.
            if stack_writes.iter().any(|&(address, width)| {
                u128::from(descriptor) >= u128::from(address)
                    && u128::from(descriptor) < u128::from(address) + width as u128
            }) {
                return Err(MemoryError::Backend("descriptor aliases frame".into()));
            }
            Ok(())
        }

        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetches += 1;
            let range = self.range(address, 1)?;
            Ok(self.bytes[range.start])
        }

        fn read(
            &mut self,
            address: u64,
            context: AccessContext,
            destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            let index = self.reads.len();
            self.reads.push((address, destination.len(), context));
            if self.fail_read == Some(index) {
                return Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(address),
                }));
            }
            destination.copy_from_slice(&self.bytes[self.range(address, destination.len())?]);
            Ok(())
        }

        fn write(
            &mut self,
            address: u64,
            context: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            let index = self.writes.len();
            self.writes.push((address, source.len(), context));
            if self.fail_write == Some(index) {
                return Err(MemoryError::Backend("injected frame store failure".into()));
            }
            let range = self.range(address, source.len())?;
            self.bytes[range].copy_from_slice(source);
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

    fn ready_state(rsp: u64) -> LongModeState {
        LongModeState {
            cr3: None,
            cr4: None,
            sti_shadow: None,
            gpr: {
                let mut gpr = [0; 16];
                gpr[crate::Register::Rsp as usize] = rsp;
                gpr
            },
            rip: 0x1234_5678_9abc,
            rflags: 0x2 | (1 << 9) | (1 << 0) | (1 << 2) | (1 << 10),
            cs_selector: Some(0x8),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9b,
                flags: 2,
            }),
            ss_selector: Some(0x10),
            ss_cache: Some(crate::StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0,
            }),
            gdtr: Some(crate::state::Gdtr {
                base: 0x1000,
                limit: 0x2f,
            }),
            idtr: Some(crate::state::Idtr {
                base: 0x2000,
                limit: 0x0fff,
            }),
            ..LongModeState::default()
        }
    }

    fn deliver(state: &mut LongModeState, ram: &mut Ram, vector: u8) -> InterruptDeliveryResult {
        deliver_maskable_interrupt(
            state,
            ram,
            vector,
            AccessContext {
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            },
            INTEGER_FLAGS,
        )
    }

    fn page_fault_state(rsp: u64) -> LongModeState {
        let mut state = ready_state(rsp);
        state.rflags = 0x2 | (1 << 0); // IF=0; arithmetic state is preserved.
        state.activity = Some(crate::CpuActivity::Running);
        state.sti_shadow = Some(crate::StiShadow::Clear);
        state.cr0 = Some(0x8001_0033);
        state.cr3 = Some(0x1000);
        state.cr4 = Some(1 << 5);
        state
    }

    fn configure_page_fault_gate(ram: &mut Ram, target_rip: u64, selector: u16) {
        let gate_at = 0x2000 + 14 * 16;
        ram.bytes[gate_at..gate_at + 2].copy_from_slice(&(target_rip as u16).to_le_bytes());
        ram.bytes[gate_at + 2..gate_at + 4].copy_from_slice(&selector.to_le_bytes());
        ram.bytes[gate_at + 4] = 0; // IST=0
        ram.bytes[gate_at + 5] = 0x8e; // present, ring-0 interrupt gate
        ram.bytes[gate_at + 6..gate_at + 8]
            .copy_from_slice(&((target_rip >> 16) as u16).to_le_bytes());
        ram.bytes[gate_at + 8..gate_at + 12]
            .copy_from_slice(&((target_rip >> 32) as u32).to_le_bytes());
        ram.bytes[gate_at + 12..gate_at + 16].fill(0);
        let descriptor_at = 0x1000 + usize::from(selector & !7);
        ram.bytes[descriptor_at..descriptor_at + 8]
            .copy_from_slice(&[0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0]);
    }

    fn page_fault_record(
        state: &LongModeState,
        context: AccessContext,
    ) -> crate::memory::PageFaultRecord {
        crate::memory::PageFaultRecord::new(state.rip, AccessKind::Write, context, 3, 0x4000)
    }

    fn deliver_page_fault_for_test(
        state: &mut LongModeState,
        ram: &mut Ram,
        record: &crate::memory::PageFaultRecord,
        context: AccessContext,
    ) -> PageFaultDeliveryResult {
        deliver_page_fault(state, ram, record, context)
    }

    #[test]
    fn active_sti_shadow_rejects_delivery_before_descriptor_or_stack_access() {
        let mut state = ready_state(0x8000);
        state.sti_shadow = Some(crate::StiShadow::Blocking);
        let before = state.clone();
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::InterruptsInhibited)
        );
        assert_eq!(state, before);
        assert!(ram.reads.is_empty());
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn cr0_state_rejects_interrupt_delivery_before_any_memory_callback() {
        let mut state = ready_state(0x8000);
        state.cr0 = Some(crate::execute::Cr0Profile::BASE);
        let before = state.clone();
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::UnsupportedCr0State)
        );
        assert_eq!(state, before);
        assert_eq!(ram.fetches, 0);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }

    #[test]
    fn halted_delivery_requires_the_profiled_clear_shadow() {
        let mut state = ready_state(0x8000);
        state.activity = Some(crate::CpuActivity::Halted);
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(
                InterruptOutsideProfile::HaltedWithoutClearShadow
            )
        );
        assert!(ram.reads.is_empty());
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn cpu_owned_cr3_rejects_interrupt_delivery_before_any_memory_callback() {
        let mut state = ready_state(0x8000);
        state.cr3 = Some(0x1000);
        let before = state.clone();
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::UnsupportedCr3State)
        );
        assert_eq!(state, before);
        assert_eq!(ram.fetches, 0);
        assert!(ram.reads.is_empty());
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn successful_delivery_from_halted_state_saves_post_hlt_rip_and_wakes() {
        let mut state = ready_state(0x8000);
        let post_hlt_rip = state.rip + 1;
        state.rip = post_hlt_rip;
        state.activity = Some(crate::CpuActivity::Halted);
        state.sti_shadow = Some(crate::StiShadow::Clear);
        let mut ram = Ram::new();
        let handler = 0x3000;
        ram.configure(0x20, handler, 0x9b, 0x2);
        ram.bytes[handler as usize] = 0x90;
        assert!(matches!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::Delivered { target_rip, .. } if target_rip == handler
        ));
        assert_eq!(state.activity, Some(crate::CpuActivity::Running));
        let old_rip_slot = (0x8000 & !15) - 40;
        assert_eq!(
            u64::from_le_bytes(
                ram.bytes[old_rip_slot as usize..old_rip_slot as usize + 8]
                    .try_into()
                    .unwrap()
            ),
            post_hlt_rip
        );
        assert_eq!(
            crate::execute::step(
                &mut state,
                &mut ram,
                AccessContext {
                    privilege: PrivilegeLevel::Ring0,
                    ..AccessContext::default()
                }
            )
            .unwrap(),
            crate::execute::StepOutcome::Advanced
        );
        assert_eq!(state.rip, handler + 1);
        assert_eq!(state.activity, Some(crate::CpuActivity::Running));
    }

    fn paged_state(rsp: u64) -> LongModeState {
        LongModeState {
            cr0: Some(crate::execute::Cr0Profile::BASE),
            cr3: Some(0x1000),
            cr4: Some(crate::execute::Cr4Profile::BASE),
            ..ready_state(rsp)
        }
    }

    fn deliver_paged(
        state: &mut LongModeState,
        ram: &mut Ram,
        efer: u64,
    ) -> InterruptDeliveryResult {
        deliver_paged_maskable_interrupt(
            state,
            ram,
            0x20,
            AccessContext {
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            },
            INTEGER_FLAGS,
            efer,
        )
    }

    #[test]
    fn paged_front_writes_the_flat_frame_as_implicit_stack_qwords() {
        let mut state = paged_state(0x8008);
        let before = state.clone();
        let mut ram = Ram::new();
        ram.configure(0x20, 0xffff_8000_1234_5678, 0x9b, 0x2);
        assert_eq!(
            deliver_paged(&mut state, &mut ram, 0xd00),
            InterruptDeliveryResult::Delivered {
                vector: 0x20,
                target_rip: 0xffff_8000_1234_5678,
                new_rsp: 0x8000 - 40,
            }
        );
        assert_eq!(
            ram.writes.iter().map(|w| w.0).collect::<Vec<_>>(),
            [0x7ff8, 0x7ff0, 0x7fe8, 0x7fe0, 0x7fd8]
        );
        assert!(
            ram.writes
                .iter()
                .all(|(_, len, ctx)| { *len == 8 && ctx.origin == AccessOrigin::ImplicitStack })
        );
        assert_eq!(
            &ram.bytes[0x7fe8..0x7ff0],
            &before.rflags.to_le_bytes(),
            "saved RFLAGS keeps IF"
        );
        assert_eq!(state.cr0, before.cr0);
        assert_eq!(state.cr3, before.cr3);
        assert_eq!(state.rflags & (1 << 9), 0);
    }

    #[test]
    fn paged_front_rejects_unsupported_controls_before_any_access() {
        let efer_bad = |efer| {
            let mut state = paged_state(0x8000);
            let mut ram = Ram::new();
            let result = deliver_paged(&mut state, &mut ram, efer);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
            result
        };
        assert_eq!(
            efer_bad(0x500),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::UnsupportedCr3State)
        );
        for mutate in [
            |s: &mut LongModeState| s.cr3 = None,
            |s: &mut LongModeState| s.cr3 = Some(1 << 36),
            |s: &mut LongModeState| s.cr0 = None,
            |s: &mut LongModeState| s.cr0 = Some(0x8005_0033),
            |s: &mut LongModeState| s.cr4 = Some(0x20 | (1 << 9)),
        ] {
            let mut state = paged_state(0x8000);
            mutate(&mut state);
            let before = state.clone();
            let mut ram = Ram::new();
            assert!(matches!(
                deliver_paged(&mut state, &mut ram, 0xd00),
                InterruptDeliveryResult::OutsideProfile(
                    InterruptOutsideProfile::UnsupportedCr0State
                        | InterruptOutsideProfile::UnsupportedCr3State
                )
            ));
            assert_eq!(state, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
        // The flat front still refuses a paged state.
        let mut state = paged_state(0x8000);
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::UnsupportedCr3State)
        );
    }

    #[test]
    fn delivery_writes_exact_frame_and_commits_only_after_all_stores() {
        for rsp in [
            0x8000, 0x8001, 0x8007, 0x8008, 0x800f, 0x8010, 0x8017, 0x801f,
        ] {
            let mut state = ready_state(rsp);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0xffff_8000_1234_5678, 0x9b, 0x2);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::Delivered {
                    vector: 0x20,
                    target_rip: 0xffff_8000_1234_5678,
                    new_rsp: (rsp & !15) - 40,
                }
            );
            let old_rsp = before.gpr[crate::Register::Rsp as usize];
            let aligned = old_rsp & !15;
            let expected = [
                (aligned - 8, before.ss_selector.unwrap() as u64),
                (aligned - 16, old_rsp),
                (aligned - 24, before.rflags),
                (aligned - 32, before.cs_selector.unwrap() as u64),
                (aligned - 40, before.rip),
            ];
            assert_eq!(
                ram.writes.iter().map(|w| w.0).collect::<Vec<_>>(),
                expected.iter().map(|e| e.0).collect::<Vec<_>>()
            );
            for (address, value) in expected {
                assert_eq!(
                    &ram.bytes[address as usize..address as usize + 8],
                    &value.to_le_bytes()
                );
            }
            assert_eq!(state.cs_selector, Some(0x20));
            assert_eq!(
                state.cs_cache,
                Some(CodeSegmentDescriptorCache {
                    base: 0,
                    limit: u32::MAX,
                    access: 0x9b,
                    flags: 2,
                })
            );
            assert_eq!(state.ss_selector, before.ss_selector);
            assert_eq!(state.ss_cache, before.ss_cache);
            assert_eq!(state.gpr[crate::Register::Rsp as usize], (rsp & !15) - 40);
            assert_eq!(
                state.rflags,
                before.rflags & !((1 << 9) | (1 << 8) | (1 << 17) | (1 << 16) | (1 << 14))
            );
            assert_eq!(state.rip, 0xffff_8000_1234_5678);
            assert_eq!(ram.reads.len(), 2);
            assert_eq!(ram.fetches, 0);
            assert!(
                ram.reads
                    .iter()
                    .all(|(_, _, ctx)| ctx.origin == AccessOrigin::DescriptorTable)
            );
        }
    }

    #[test]
    fn dpl_and_rpl_do_not_mask_external_vectors_including_2_and_14() {
        for vector in [2, 14] {
            let mut state = ready_state(0x8000);
            let mut ram = Ram::new();
            ram.configure(vector, 0xffff_8000_1234_5678, 0x9b, 0x2);
            let gate = 0x2000 + usize::from(vector) * 16;
            ram.bytes[gate + 5] = 0xee; // DPL 3, type 0xe, present
            ram.bytes[gate + 2..gate + 4].copy_from_slice(&0x23u16.to_le_bytes());
            ram.bytes[0x1000 + 0x25] = 0x9b; // target RPL ignored
            assert!(matches!(
                deliver(&mut state, &mut ram, vector),
                InterruptDeliveryResult::Delivered { .. }
            ));
        }
    }

    #[test]
    fn null_ss_selector_without_cache_is_preserved_for_long64_delivery() {
        let mut state = ready_state(0x8000);
        state.ss_selector = Some(0);
        state.ss_cache = None;
        let mut ram = Ram::new();
        ram.configure(0x20, 0x1234, 0x9b, 0x2);
        assert!(matches!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::Delivered { .. }
        ));
        assert_eq!(state.ss_selector, Some(0));
        assert_eq!(state.ss_cache, None);
    }

    #[test]
    fn every_stack_store_failure_keeps_cpu_state_and_prior_memory_writes() {
        for fail_at in 0..5 {
            let mut state = ready_state(0x8007);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234_5678, 0x9b, 0x2);
            ram.fail_write = Some(fail_at);
            let result = deliver(&mut state, &mut ram, 0x20);
            assert!(matches!(
                result,
                InterruptDeliveryResult::DeliveryAborted {
                    phase: InterruptDeliveryPhase::StackWrite { .. },
                    ..
                }
            ));
            assert_eq!(state, before);
            assert_eq!(ram.writes.len(), fail_at + 1);
            for (address, _, _) in ram.writes.iter().take(fail_at) {
                assert_ne!(
                    &ram.bytes[*address as usize..*address as usize + 8],
                    &[0; 8]
                );
            }
        }
    }

    #[test]
    fn bad_gate_or_target_and_descriptor_read_fault_never_touch_stack() {
        for (access, flags, expected) in [
            (
                0x93,
                0x2,
                InterruptOutsideProfile::InvalidTargetCodeDescriptor,
            ),
            (
                0x9b,
                0x4,
                InterruptOutsideProfile::InvalidTargetCodeDescriptor,
            ),
        ] {
            let mut state = ready_state(0x8000);
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, access, flags);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(expected)
            );
            assert!(ram.writes.is_empty());
        }
        let mut state = ready_state(0x8000);
        let mut ram = Ram::new();
        ram.configure(0x20, 0x1234, 0x9b, 0x2);
        ram.fail_read = Some(1);
        let before = state.clone();
        assert!(matches!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::DeliveryAborted {
                phase: InterruptDeliveryPhase::TargetCodeDescriptorRead,
                cause: MemoryError::Architectural(_)
            }
        ));
        assert_eq!(state, before);
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn invalid_gate_reserved_type_present_and_ist_fields_do_not_write_frame() {
        let mutations: [(&str, fn(&mut [u8])); 6] = [
            ("system bit", |gate: &mut [u8]| gate[5] |= 0x10),
            ("reserved type", |gate: &mut [u8]| gate[5] = 0x8f),
            ("not present", |gate: &mut [u8]| gate[5] &= !0x80),
            ("IST nonzero", |gate: &mut [u8]| gate[4] = 1),
            ("IST reserved", |gate: &mut [u8]| gate[4] = 8),
            ("reserved dword", |gate: &mut [u8]| gate[12] = 1),
        ];
        for (label, mutate) in mutations {
            let mut state = ready_state(0x8000);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, 0x9b, 0x2);
            let gate_at = 0x2000 + 0x20 * 16;
            mutate(&mut ram.bytes[gate_at..gate_at + 16]);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::InvalidGate),
                "{}",
                label
            );
            assert_eq!(state, before);
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn null_or_ti_set_gate_selectors_are_rejected_before_gdt_or_stack_access() {
        for selector in [0x0003u16, 0x0024] {
            let mut state = ready_state(0x8000);
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, 0x9b, 0x2);
            let gate_at = 0x2000 + 0x20 * 16;
            ram.bytes[gate_at + 2..gate_at + 4].copy_from_slice(&selector.to_le_bytes());
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(
                    InterruptOutsideProfile::InvalidTargetSelector
                )
            );
            assert_eq!(ram.reads.len(), 1);
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn incoherent_current_cs_selector_or_long64_cache_is_rejected_before_idt_read() {
        for (selector, access, flags, base, limit) in [
            (0, 0x9b, 0x2, 0, u32::MAX),
            (0x9, 0x9b, 0x2, 0, u32::MAX),
            (0x8, 0x93, 0x2, 0, u32::MAX),
            (0x8, 0x9f, 0x2, 0, u32::MAX),
            (0x8, 0xbb, 0x2, 0, u32::MAX),
            (0x8, 0x1b, 0x2, 0, u32::MAX),
            (0x8, 0x9b, 0, 0, u32::MAX),
            (0x8, 0x9b, 0x4, 0, u32::MAX),
            (0x8, 0x9b, 0x6, 0, u32::MAX),
            (0x8, 0x9b, 0x2, 1, u32::MAX),
            (0x8, 0x9b, 0x2, 0, 0xffff),
        ] {
            let mut state = ready_state(0x8000);
            state.cs_selector = Some(selector);
            state.cs_cache = Some(CodeSegmentDescriptorCache {
                base,
                limit,
                access,
                flags,
            });
            let before = state.clone();
            let mut ram = Ram::new();
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(
                    InterruptOutsideProfile::IncompatibleCurrentSelectors
                )
            );
            assert_eq!(state, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }

    #[test]
    fn gdt_limit_and_stack_canonicality_or_underflow_preflight_without_frame_writes() {
        let mut state = ready_state(0x8000);
        state.gdtr = Some(crate::state::Gdtr {
            base: 0x1000,
            limit: 0x26,
        });
        let mut ram = Ram::new();
        ram.configure(0x20, 0x1234, 0x9b, 0x2);
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(
                InterruptOutsideProfile::MissingTargetDescriptor
            )
        );
        assert_eq!(ram.reads.len(), 1);
        assert!(ram.writes.is_empty());

        for (rsp, expected) in [
            (0x20, InterruptOutsideProfile::StackAddressOverflow),
            (
                0x0000_8000_0000_0010,
                InterruptOutsideProfile::NonCanonicalStackSpan,
            ),
            (
                0xffff_8000_0000_0010,
                InterruptOutsideProfile::NonCanonicalStackSpan,
            ),
        ] {
            let mut state = ready_state(rsp);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, 0x9b, 0x2);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(expected)
            );
            assert_eq!(state, before);
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn missing_idt_bounds_and_noncanonical_spans_do_not_read_or_write_frame() {
        let mut state = ready_state(0x8000);
        state.idtr = Some(crate::state::Idtr {
            base: 0x2000,
            limit: 0x1ff,
        });
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::MissingIdtEntry)
        );
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
        state.idtr = Some(crate::state::Idtr {
            base: 0x0000_7fff_ffff_fff8,
            limit: 0xffff,
        });
        assert_eq!(
            deliver(&mut state, &mut ram, 1),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::NonCanonicalIdtSpan)
        );
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }

    #[test]
    fn page_fault_delivery_pushes_exact_six_qwords_and_sets_rf_only_in_saved_image() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let original_rsp = 0x8007;
        let target_rip = 0x3000;
        let mut state = page_fault_state(original_rsp);
        let before = state.clone();
        let record = page_fault_record(&state, context);
        let mut ram = Ram::new();
        configure_page_fault_gate(&mut ram, target_rip, 0x8);

        assert_eq!(
            deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::Delivered {
                target_rip,
                new_rsp: 0x7fd0,
            }
        );
        assert_eq!(state.rip, target_rip);
        assert_eq!(state.gpr[crate::Register::Rsp as usize], 0x7fd0);
        assert_eq!(state.cr2, Some(0x4000));
        assert_eq!(
            state.rflags,
            before.rflags & !((1 << 9) | (1 << 8) | (1 << 17) | (1 << 16) | (1 << 14))
        );
        assert_eq!(
            ram.writes.iter().map(|entry| entry.0).collect::<Vec<_>>(),
            [0x7ff8, 0x7ff0, 0x7fe8, 0x7fe0, 0x7fd8, 0x7fd0]
        );
        let saved_flags = before.rflags | (1 << 16);
        let expected = [0x10, original_rsp, saved_flags, 0x8, before.rip, 3];
        for ((address, _, _), value) in ram.writes.iter().zip(expected) {
            assert_eq!(
                u64::from_le_bytes(
                    ram.bytes[*address as usize..*address as usize + 8]
                        .try_into()
                        .unwrap()
                ),
                value
            );
        }
        assert!(
            ram.reads
                .iter()
                .all(|(_, _, access)| access.origin == AccessOrigin::DescriptorTable)
        );
        assert!(
            ram.writes
                .iter()
                .all(|(_, _, access)| access.origin == AccessOrigin::ImplicitStack)
        );
        assert_eq!(ram.writes.len(), 6); // A=1 requires no descriptor write.
    }

    fn double_fault_record(
        state: &LongModeState,
        context: AccessContext,
    ) -> crate::memory::DoubleFaultRecord {
        let original = page_fault_record(state, context);
        let secondary = crate::memory::PageFaultRecord::new(
            state.rip,
            AccessKind::Read,
            context.descriptor_table(),
            0,
            0x5678,
        );
        crate::memory::DoubleFaultRecord::new(original, secondary)
    }

    fn configure_double_fault_gate(ram: &mut Ram, target: u64) {
        configure_page_fault_gate(ram, target, 0x8);
        let pf = 0x2000 + 14 * 16;
        let df = 0x2000 + 8 * 16;
        ram.bytes.copy_within(pf..pf + 16, df);
        ram.bytes[pf..pf + 16].fill(0); // #PF gate is deliberately invalid.
    }

    #[test]
    fn double_fault_selects_vector_eight_and_commits_abort_frame() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8007);
        state.cr2 = Some(0x5678);
        let before = state.clone();
        let record = double_fault_record(&state, context);
        assert_eq!(record.original().cr2_value(), 0x4000);
        assert_eq!(record.secondary().cr2_value(), 0x5678);
        let mut ram = Ram::new();
        configure_double_fault_gate(&mut ram, 0x3000);
        assert_eq!(
            deliver_double_fault(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::Delivered {
                target_rip: 0x3000,
                new_rsp: 0x7fd0
            }
        );
        assert_eq!(ram.reads[0].0, 0x2080);
        assert_eq!(state.cr2, before.cr2);
        assert_eq!(state.rip, 0x3000);
        assert_eq!(state.gpr[crate::Register::Rsp as usize], 0x7fd0);
        assert_eq!(state.cs_selector, Some(8));
        let values = [0x10, 0x8007, before.rflags, 8, before.rip, 0];
        for ((address, _, _), expected) in ram.writes.iter().zip(values) {
            assert_eq!(
                u64::from_le_bytes(
                    ram.bytes[*address as usize..*address as usize + 8]
                        .try_into()
                        .unwrap()
                ),
                expected
            );
        }
        assert_eq!(ram.writes.len(), 6);
    }

    #[test]
    fn double_fault_failures_keep_entry_state_and_classify_cause() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8000);
        state.cr2 = Some(0x5678);
        let before = state.clone();
        let record = double_fault_record(&state, context);
        let mut ram = Ram::new();
        configure_double_fault_gate(&mut ram, 0x3000);
        ram.fail_write = Some(2);
        assert!(matches!(
            deliver_double_fault(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::DeliveryAborted {
                phase: PageFaultDeliveryPhase::StackWrite {
                    slot: PageFaultFrameSlot::OldRflags
                },
                cause: MemoryError::Backend(_)
            }
        ));
        assert_eq!(state, before);
        assert_eq!(ram.writes.len(), 3);
        let mut ram = Ram::new();
        assert_eq!(
            deliver_double_fault(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::OutsideProfile(PageFaultDeliveryOutsideProfile::InvalidGate)
        );
        assert_eq!(state, before);
        let mut ram = Ram::new();
        ram.fail_read = Some(0);
        assert!(matches!(
            deliver_double_fault(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::DeliveryAborted {
                phase: PageFaultDeliveryPhase::IdtGateRead,
                cause: MemoryError::Architectural(_)
            }
        ));
        assert_eq!(state.cr2, Some(0x2080));
        assert_eq!(state.rip, before.rip);
        assert_eq!(state.gpr, before.gpr);
    }

    #[test]
    fn stale_record_is_rejected_before_cr2_or_any_memory_access() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8000);
        let mut record = page_fault_record(&state, context);
        state.rip += 1;
        state.cr2 = Some(0x1234);
        let mut ram = Ram::new();
        assert_eq!(
            deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::OutsideProfile(
                PageFaultDeliveryOutsideProfile::FaultRipMismatch
            )
        );
        assert_eq!(state.cr2, Some(0x1234));
        assert!(ram.reads.is_empty() && ram.writes.is_empty());

        record = page_fault_record(&state, context);
        let mut unsupported = state.clone();
        unsupported.cr4 = None;
        let mut ram = Ram::new();
        assert!(matches!(
            deliver_page_fault_for_test(&mut unsupported, &mut ram, &record, context),
            PageFaultDeliveryResult::OutsideProfile(
                PageFaultDeliveryOutsideProfile::MissingPagingState
            )
        ));
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }

    #[test]
    fn page_fault_delivery_rejects_halted_or_sti_shadow_state_before_side_effects() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        for (activity, shadow, expected) in [
            (
                Some(crate::CpuActivity::Halted),
                Some(crate::StiShadow::Clear),
                PageFaultDeliveryOutsideProfile::ActivityOutsideProfile,
            ),
            (
                Some(crate::CpuActivity::Running),
                Some(crate::StiShadow::Blocking),
                PageFaultDeliveryOutsideProfile::StiShadowActive,
            ),
        ] {
            let mut state = page_fault_state(0x8000);
            state.activity = activity;
            state.sti_shadow = shadow;
            state.cr2 = Some(0x1234);
            let before = state.clone();
            let record = page_fault_record(&state, context);
            let mut ram = Ram::new();
            assert_eq!(
                deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
                PageFaultDeliveryResult::OutsideProfile(expected)
            );
            assert_eq!(state, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }

    #[test]
    fn implicit_secondary_stack_fault_cannot_be_redelivered_as_a_new_original_fault() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::ImplicitStack,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8000);
        let record = page_fault_record(&state, context);
        state.cr2 = Some(0x1234);
        let before = state.clone();
        let mut ram = Ram::new();
        assert_eq!(
            deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::OutsideProfile(
                PageFaultDeliveryOutsideProfile::NotExplicitInstructionFault
            )
        );
        assert_eq!(state, before);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }

    #[test]
    fn secondary_page_fault_updates_cr2_and_retains_terminal_phase() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        for (fail_read, phase, expected_cr2) in [
            (0, PageFaultDeliveryPhase::IdtGateRead, 0x20e0),
            (1, PageFaultDeliveryPhase::TargetCodeDescriptorRead, 0x1008),
        ] {
            let mut state = page_fault_state(0x8000);
            let record = page_fault_record(&state, context);
            let mut ram = Ram::new();
            configure_page_fault_gate(&mut ram, 0x3000, 0x8);
            ram.fail_read = Some(fail_read);
            assert!(matches!(
                deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
                PageFaultDeliveryResult::DeliveryAborted { phase: actual, .. } if actual == phase
            ));
            assert_eq!(state.cr2, Some(expected_cr2));
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn target_ti_selector_is_explicitly_outside_profile() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        for selector in [0x0c] {
            let mut state = page_fault_state(0x8000);
            let record = page_fault_record(&state, context);
            let mut ram = Ram::new();
            configure_page_fault_gate(&mut ram, 0x3000, selector);
            assert_eq!(
                deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
                PageFaultDeliveryResult::OutsideProfile(
                    PageFaultDeliveryOutsideProfile::InvalidTargetSelector
                )
            );
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn unaccessed_target_descriptor_is_written_after_frame_and_cached_as_accessed() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8000);
        let record = page_fault_record(&state, context);
        let mut ram = Ram::new();
        configure_page_fault_gate(&mut ram, 0x3000, 0x8);
        ram.bytes[0x100d] = 0x9a;

        assert_eq!(
            deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::Delivered {
                target_rip: 0x3000,
                new_rsp: 0x7fd0,
            }
        );
        assert_eq!(ram.writes.len(), 7);
        assert_eq!(
            ram.writes[..6]
                .iter()
                .map(|(address, _, _)| *address)
                .collect::<Vec<_>>(),
            [0x7ff8, 0x7ff0, 0x7fe8, 0x7fe0, 0x7fd8, 0x7fd0]
        );
        assert_eq!(ram.writes[6].0, 0x100d);
        assert_eq!(ram.writes[6].1, 1);
        assert_eq!(ram.writes[6].2.origin, AccessOrigin::DescriptorTable);
        assert_eq!(ram.bytes[0x100d], 0x9b);
        assert_eq!(
            state.cs_cache.unwrap().access,
            0x9b,
            "CS cache must reflect the descriptor after its A-bit update"
        );
        assert_eq!(state.rip, 0x3000);
    }

    #[test]
    fn failed_target_descriptor_accessed_write_keeps_frame_but_does_not_commit_entry() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8000);
        let mut expected_state = state.clone();
        expected_state.cr2 = Some(0x4000);
        let record = page_fault_record(&state, context);
        let mut ram = Ram::new();
        configure_page_fault_gate(&mut ram, 0x3000, 0x8);
        ram.bytes[0x100d] = 0x9a;
        ram.fail_write = Some(6);

        assert!(matches!(
            deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::DeliveryAborted {
                phase: PageFaultDeliveryPhase::TargetCodeDescriptorAccessedWrite,
                cause: MemoryError::Backend(_),
            }
        ));
        assert_eq!(state, expected_state);
        assert_eq!(ram.writes.len(), 7);
        assert_eq!(ram.bytes[0x100d], 0x9a);
        for (address, _, _) in &ram.writes[..6] {
            assert_ne!(
                &ram.bytes[*address as usize..*address as usize + 8],
                &[0; 8]
            );
        }
    }

    #[test]
    fn failed_stack_store_preserves_prefix_and_does_not_claim_double_fault() {
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let mut state = page_fault_state(0x8000);
        let before = state.clone();
        let record = page_fault_record(&state, context);
        let mut ram = Ram::new();
        configure_page_fault_gate(&mut ram, 0x3000, 0x8);
        ram.fail_write = Some(3);
        assert!(matches!(
            deliver_page_fault_for_test(&mut state, &mut ram, &record, context),
            PageFaultDeliveryResult::DeliveryAborted {
                phase: PageFaultDeliveryPhase::StackWrite {
                    slot: PageFaultFrameSlot::OldCs
                },
                cause: MemoryError::Backend(_),
            }
        ));
        assert_eq!(state.rip, before.rip);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.cr2, Some(record.cr2_value()));
        assert_eq!(ram.writes.len(), 4);
        for (address, _, _) in &ram.writes[..3] {
            assert_ne!(
                &ram.bytes[*address as usize..*address as usize + 8],
                &[0; 8]
            );
        }
    }
}
