//! Raw runtime boundary shared by future block entries and their host publisher.
//!
//! These tags are integer ABI values. `LongModeState` remains an opaque pointer:
//! generated code may use the offsets below only when compiled with this crate.

use od_isa::long_decode::MemoryOperand;
use od_isa::{LongModeState, integer_flags};

pub const LONG_MODE_GPR_OFFSET: usize = std::mem::offset_of!(LongModeState, gpr);
pub const LONG_MODE_RIP_OFFSET: usize = std::mem::offset_of!(LongModeState, rip);
pub const LONG_MODE_RFLAGS_OFFSET: usize = std::mem::offset_of!(LongModeState, rflags);

pub const RUNTIME_CONTEXT_STATE_OFFSET: usize = std::mem::offset_of!(RuntimeContextV1, state);
pub const RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV1, initial_budget);
pub const RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV1, stop_rip_enabled);
pub const RUNTIME_CONTEXT_STOP_RIP_OFFSET: usize = std::mem::offset_of!(RuntimeContextV1, stop_rip);
pub const RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV1, stop_requested);
pub const RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV1, remaining_budget);
pub const RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV1, retired_instructions);

/// Raw entry result reserved for an unaccepted helper status. It is not a
/// `RuntimeExitTag` and must be rejected by the host boundary.
pub const RAW_HELPER_CONTRACT_FAILURE_V1: u32 = 0xFFFF_FFFE;

/// Versioned raw call record. The state pointer is opaque and same-build only;
/// all other members are fixed-width integers or opaque pointers.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RuntimeContextV1 {
    pub state: *mut LongModeState,
    pub initial_budget: u64,
    /// 0 disables the stop address; 1 enables `stop_rip`; all other values fail.
    pub stop_rip_enabled: u32,
    pub reserved: u32,
    pub stop_rip: u64,
    /// Required pointer to an aligned atomic byte; zero means no cooperative stop.
    pub stop_requested: *const u8,
    /// Output: budget remaining after this invocation.
    pub remaining_budget: u64,
    /// Output: instructions retired in this invocation.
    pub retired_instructions: u64,
}

/// Block entry takes a non-null `RuntimeContextV1` pointer in x0 and returns a
/// raw tag in w0. It follows AAPCS64: x0-x17, x30, NZCV and caller-clobbered
/// SIMD state are volatile; x19-x29 and SP are preserved. Generated code must
/// not use Apple-reserved x18 or unwind through a generated frame.
pub type BlockEntry = unsafe extern "C" fn(*mut RuntimeContextV1) -> u32;

/// ABI for the first production checked-memory block profile. This remains a
/// separate entry type so V1 blocks can never be called with a V2 context.
pub type BlockEntryV2 = unsafe extern "C" fn(*mut RuntimeContextV2) -> u32;

/// V2 runtime return for a checked memory operation which did not complete.
/// It is not a `RuntimeExitTag`; the host decodes the invocation-local outcome.
pub const RUNTIME_EXIT_V2_MEMORY_OPERATION: u32 = 9;

/// The checked memory operation encoded by a V2 site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemorySiteOperationV2 {
    Load,
    Store,
}

/// Immutable per-block description for one verifier-approved memory operation.
/// The pointed-to representation is private to the same linked build; it is
/// never passed as a C-layout record or retained by the translation key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemorySiteV2 {
    pub(crate) instruction_index: u32,
    pub(crate) guest_rip: u64,
    pub(crate) next_rip: u64,
    pub(crate) operation: MemorySiteOperationV2,
    /// Exactly one of source/destination is present for this MOV profile.
    pub(crate) destination: Option<u8>,
    pub(crate) source: Option<u8>,
    pub(crate) operand: MemoryOperand,
}

impl MemorySiteV2 {
    pub const fn instruction_index(self) -> u32 {
        self.instruction_index
    }

    pub const fn guest_rip(self) -> u64 {
        self.guest_rip
    }

    pub const fn next_rip(self) -> u64 {
        self.next_rip
    }

    pub const fn destination(self) -> Option<u8> {
        self.destination
    }

    pub const fn source(self) -> Option<u8> {
        self.source
    }

    pub const fn operation(self) -> MemorySiteOperationV2 {
        self.operation
    }

    pub const fn operand(self) -> MemoryOperand {
        self.operand
    }
}

/// V2 call record. Its first member preserves the V1 control prefix, while
/// the additional invocation and site pointers are valid only for this call.
/// Never pass this record to a V1 `BlockEntry`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RuntimeContextV2 {
    pub base: RuntimeContextV1,
    pub memory_adapter: *mut std::ffi::c_void,
    pub memory_sites: *const MemorySiteV2,
    pub memory_site_count: u32,
    /// Output: site index used by the last helper call, or `u32::MAX`.
    pub last_memory_site: u32,
    /// Output: raw status in the separate CheckedMemoryHelperV1 namespace.
    pub last_memory_status: u32,
    pub reserved: u32,
    /// Staged load value, valid only when `last_memory_status == 0`.
    pub staged_load_value: u64,
    /// Output payload written by CheckedMemoryHelperV1.
    pub memory_outcome: crate::memory_bridge::CheckedMemoryOutcomeV1,
}

pub const RUNTIME_CONTEXT_V2_BASE_OFFSET: usize = std::mem::offset_of!(RuntimeContextV2, base);
pub const RUNTIME_CONTEXT_V2_MEMORY_ADAPTER_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, memory_adapter);
pub const RUNTIME_CONTEXT_V2_MEMORY_SITES_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, memory_sites);
pub const RUNTIME_CONTEXT_V2_MEMORY_SITE_COUNT_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, memory_site_count);
pub const RUNTIME_CONTEXT_V2_LAST_MEMORY_SITE_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, last_memory_site);
pub const RUNTIME_CONTEXT_V2_LAST_MEMORY_STATUS_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, last_memory_status);
pub const RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, staged_load_value);
pub const RUNTIME_CONTEXT_V2_MEMORY_OUTCOME_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV2, memory_outcome);

/// Trusted Rust helper used only by verified V2 memory-load blocks. The
/// generated code passes a site index; effective address validation remains
/// in `MemoryOperand::access_address_48` and memory access remains checked.
///
/// # Safety
/// `context` must point to a live invocation-local `RuntimeContextV2` whose
/// state, adapter, and immutable memory-site table remain valid for the call.
pub unsafe extern "C" fn memory_site_helper_v2(
    context: *mut RuntimeContextV2,
    site_index: u32,
) -> u32 {
    use crate::memory_bridge::{
        CheckedMemoryOutcomeV1, MEMORY_HELPER_INVALID_ARGUMENT, MEMORY_HELPER_SUCCESS,
        MEMORY_OP_READ, MEMORY_OP_WRITE, checked_memory_helper_v1, classify_error,
    };

    if context.is_null() {
        return MEMORY_HELPER_INVALID_ARGUMENT;
    }
    // SAFETY: required by this function's caller contract.
    let context = unsafe { &mut *context };
    context.last_memory_site = u32::MAX;
    context.last_memory_status = MEMORY_HELPER_INVALID_ARGUMENT;
    context.staged_load_value = 0;
    context.memory_outcome = CheckedMemoryOutcomeV1::default();
    context.memory_outcome.status = MEMORY_HELPER_INVALID_ARGUMENT;

    if context.memory_adapter.is_null()
        || context.memory_sites.is_null()
        || site_index >= context.memory_site_count
        || context.base.state.is_null()
    {
        context.last_memory_site = site_index;
        return MEMORY_HELPER_INVALID_ARGUMENT;
    }

    // SAFETY: site bounds and table lifetime are part of the caller contract.
    let site = unsafe { *context.memory_sites.add(site_index as usize) };
    // SAFETY: the state pointer is kept live by the V2 host wrapper.
    let state = unsafe { &*context.base.state };
    let decoded_memory_form_matches = match (
        site.next_rip.checked_sub(site.guest_rip),
        site.operand.index,
    ) {
        (Some(3), None) => site.operand.scale == 1,
        (Some(4), Some(_)) => matches!(site.operand.scale, 1 | 2 | 4 | 8),
        _ => false,
    };
    if site.instruction_index != 0
        || !matches!(site.next_rip.checked_sub(site.guest_rip), Some(3 | 4))
        || !decoded_memory_form_matches
        || state.rip != site.guest_rip
        || match site.operation {
            MemorySiteOperationV2::Load => {
                !matches!(site.destination, Some(0..=3)) || site.source.is_some()
            }
            MemorySiteOperationV2::Store => {
                !matches!(site.source, Some(0..=3)) || site.destination.is_some()
            }
        }
        || !matches!(
            site.operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rax
                    | od_isa::Register::Rcx
                    | od_isa::Register::Rdx
                    | od_isa::Register::Rbx
            )
        )
        || site.operand.index.is_some_and(|index| {
            !matches!(
                index,
                od_isa::Register::Rax
                    | od_isa::Register::Rcx
                    | od_isa::Register::Rdx
                    | od_isa::Register::Rbx
            )
        })
        || site.operand.displacement != 0
        || site.operand.address_size != od_isa::OperandSize::Qword
        || site.operand.size != od_isa::OperandSize::Qword
    {
        context.last_memory_site = site_index;
        return MEMORY_HELPER_INVALID_ARGUMENT;
    }
    let address = match site.operand.access_address_48(state, site.next_rip) {
        Ok(address) => address,
        Err(error) => {
            classify_error(error, &mut context.memory_outcome);
            context.last_memory_site = site_index;
            context.last_memory_status = context.memory_outcome.status;
            return context.last_memory_status;
        }
    };

    let mut bytes = [0_u8; 8];
    let operation = match site.operation {
        MemorySiteOperationV2::Load => MEMORY_OP_READ,
        MemorySiteOperationV2::Store => {
            let Some(source) = site.source else {
                context.last_memory_site = site_index;
                return MEMORY_HELPER_INVALID_ARGUMENT;
            };
            bytes.copy_from_slice(&state.gpr[source as usize].to_le_bytes());
            MEMORY_OP_WRITE
        }
    };
    // SAFETY: the adapter and outcome belong to this live invocation; bytes is
    // an aligned buffer, and the checked helper only reads from it for writes.
    let status = unsafe {
        checked_memory_helper_v1(
            context.memory_adapter,
            operation,
            address,
            8,
            bytes.as_mut_ptr(),
            &mut context.memory_outcome,
        )
    };
    context.last_memory_site = site_index;
    context.last_memory_status = status;
    if status == MEMORY_HELPER_SUCCESS && site.operation == MemorySiteOperationV2::Load {
        context.staged_load_value = u64::from_le_bytes(bytes);
    }
    status
}

/// Raw block exit values. Unknown values are host/runtime contract failures.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeExitTag {
    BlockComplete = 0,
    BudgetExhausted = 1,
    NeedInterpreter = 2,
    StopRequested = 3,
    StopAddress = 4,
    Unsupported = 5,
    /// A Jcc selected and committed a guest RIP for the dispatcher. The
    /// caller must use the returned canonical RIP and must not retry the Jcc.
    DispatchCandidate = 8,
}

impl TryFrom<u32> for RuntimeExitTag {
    type Error = u32;

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        match raw {
            0 => Ok(Self::BlockComplete),
            1 => Ok(Self::BudgetExhausted),
            2 => Ok(Self::NeedInterpreter),
            3 => Ok(Self::StopRequested),
            4 => Ok(Self::StopAddress),
            5 => Ok(Self::Unsupported),
            8 => Ok(Self::DispatchCandidate),
            _ => Err(raw),
        }
    }
}

/// Tags reserved for a later runtime contract version; invalid in v1.
pub const RESERVED_GUEST_FAULT_EXIT_V1: u32 = 6;
pub const RESERVED_EVENT_PENDING_EXIT_V1: u32 = 7;

/// Scalar helper operation tags; unknown values return `HelperStatusTag::Unsupported`.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelperOperationTag {
    Add = 1,
    Subtract = 2,
    Compare = 3,
}

impl TryFrom<u32> for HelperOperationTag {
    type Error = u32;

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        match raw {
            1 => Ok(Self::Add),
            2 => Ok(Self::Subtract),
            3 => Ok(Self::Compare),
            _ => Err(raw),
        }
    }
}

/// Raw scalar helper return status values.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelperStatusTag {
    Success = 0,
    Unsupported = 1,
    InvalidArgument = 2,
}

impl TryFrom<u32> for HelperStatusTag {
    type Error = u32;

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        match raw {
            0 => Ok(Self::Success),
            1 => Ok(Self::Unsupported),
            2 => Ok(Self::InvalidArgument),
            _ => Err(raw),
        }
    }
}

/// Scalar helper ABI: operation in w0, inputs in x1-x3, and a writable output
/// pointer in x4. Status 0 is success; every other status is a host-contract
/// failure, and its output is valid only on success. The caller treats
/// x0-x17, x30 and NZCV as clobbered; the helper follows AAPCS64 and preserves
/// x19-x29 and SP. x18 is reserved on Apple.
pub type ArithmeticFlagsHelper = unsafe extern "C" fn(u32, u64, u64, u64, *mut u64) -> u32;

/// The single JIT-side adapter to shared 64-bit x86 arithmetic flag semantics.
///
/// # Safety
/// `output` must be null or point to one writable, aligned `u64` that remains
/// valid for the call. It is written only for a recognized operation.
pub unsafe extern "C" fn arithmetic_flags_helper_adapter(
    operation: u32,
    lhs: u64,
    rhs: u64,
    old_rflags: u64,
    output: *mut u64,
) -> u32 {
    if output.is_null() {
        return HelperStatusTag::InvalidArgument as u32;
    }
    let Ok(operation) = HelperOperationTag::try_from(operation) else {
        return HelperStatusTag::Unsupported as u32;
    };
    let operation = match operation {
        HelperOperationTag::Add => integer_flags::ArithmeticFlagOperation::Add,
        HelperOperationTag::Subtract => integer_flags::ArithmeticFlagOperation::Subtract,
        HelperOperationTag::Compare => integer_flags::ArithmeticFlagOperation::Compare,
    };
    let updated = integer_flags::updated_arithmetic_flags(operation, lhs, rhs, old_rflags);
    // SAFETY: the caller guarantees that every non-null output points to a
    // live, aligned, writable u64 for this invocation.
    unsafe { output.write(updated) };
    HelperStatusTag::Success as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_tags_accept_only_declared_values() {
        assert_eq!(RuntimeExitTag::try_from(4), Ok(RuntimeExitTag::StopAddress));
        assert_eq!(
            RuntimeExitTag::try_from(RESERVED_GUEST_FAULT_EXIT_V1),
            Err(RESERVED_GUEST_FAULT_EXIT_V1)
        );
        assert_eq!(
            RuntimeExitTag::try_from(RESERVED_EVENT_PENDING_EXIT_V1),
            Err(RESERVED_EVENT_PENDING_EXIT_V1)
        );
        assert_eq!(
            RuntimeExitTag::try_from(RuntimeExitTag::DispatchCandidate as u32),
            Ok(RuntimeExitTag::DispatchCandidate)
        );
        assert_eq!(
            HelperOperationTag::try_from(3),
            Ok(HelperOperationTag::Compare)
        );
        assert_eq!(HelperOperationTag::try_from(0), Err(0));
        assert_eq!(HelperStatusTag::try_from(0), Ok(HelperStatusTag::Success));
        assert_eq!(HelperStatusTag::try_from(3), Err(3));
    }

    #[test]
    fn arithmetic_helper_adapter_writes_only_on_success() {
        let mut output = 0xDEAD_BEEF_CAFE_BABE;
        // SAFETY: output is an aligned writable local u64.
        let status = unsafe {
            arithmetic_flags_helper_adapter(
                HelperOperationTag::Add as u32,
                u64::MAX,
                1,
                0xA5A5_F0F0_5A5A_0A0A,
                &mut output,
            )
        };
        assert_eq!(status, HelperStatusTag::Success as u32);
        assert_eq!(
            output,
            (0xA5A5_F0F0_5A5A_0A0A & !integer_flags::DEFINED_ARITHMETIC_FLAGS) | 0x55
        );

        output = 0xDEAD_BEEF_CAFE_BABE;
        // SAFETY: output is an aligned writable local u64.
        let status = unsafe { arithmetic_flags_helper_adapter(99, 1, 2, 3, &mut output) };
        assert_eq!(status, HelperStatusTag::Unsupported as u32);
        assert_eq!(output, 0xDEAD_BEEF_CAFE_BABE);

        // SAFETY: null is explicitly accepted and rejected without dereference.
        assert_eq!(
            unsafe { arithmetic_flags_helper_adapter(1, 1, 2, 3, std::ptr::null_mut()) },
            HelperStatusTag::InvalidArgument as u32
        );
    }

    #[test]
    fn canonical_state_offsets_are_inside_the_same_build_layout() {
        assert_eq!(
            LONG_MODE_GPR_OFFSET,
            std::mem::offset_of!(LongModeState, gpr)
        );
        assert_eq!(
            LONG_MODE_RIP_OFFSET,
            std::mem::offset_of!(LongModeState, rip)
        );
        assert_eq!(
            LONG_MODE_RFLAGS_OFFSET,
            std::mem::offset_of!(LongModeState, rflags)
        );
        assert!(LONG_MODE_GPR_OFFSET < std::mem::size_of::<LongModeState>());
        assert!(LONG_MODE_RIP_OFFSET < std::mem::size_of::<LongModeState>());
        assert!(LONG_MODE_RFLAGS_OFFSET < std::mem::size_of::<LongModeState>());
    }

    #[test]
    fn runtime_context_has_c_layout_and_non_option_stop_encoding() {
        let pointer_size = std::mem::size_of::<usize>();
        assert_eq!(std::mem::offset_of!(RuntimeContextV1, state), 0);
        assert_eq!(
            std::mem::offset_of!(RuntimeContextV1, initial_budget),
            pointer_size
        );
        assert_eq!(
            std::mem::offset_of!(RuntimeContextV1, stop_rip_enabled),
            pointer_size + 8
        );
        assert_eq!(
            std::mem::offset_of!(RuntimeContextV1, stop_rip),
            pointer_size + 16
        );
        assert_eq!(
            std::mem::offset_of!(RuntimeContextV1, stop_requested),
            pointer_size + 24
        );
        assert_eq!(
            std::mem::offset_of!(RuntimeContextV1, remaining_budget),
            2 * pointer_size + 24
        );
        assert_eq!(
            std::mem::offset_of!(RuntimeContextV1, retired_instructions),
            2 * pointer_size + 32
        );
        assert_eq!(
            std::mem::size_of::<RuntimeContextV1>(),
            2 * pointer_size + 40
        );
        assert_eq!(RuntimeExitTag::StopAddress as u32, 4);
        assert_eq!(RUNTIME_CONTEXT_STATE_OFFSET, 0);
        assert_eq!(RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET, pointer_size);
        assert_eq!(RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET, pointer_size + 8);
        assert_eq!(RUNTIME_CONTEXT_STOP_RIP_OFFSET, pointer_size + 16);
        assert_eq!(RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET, pointer_size + 24);
        assert_eq!(
            RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET,
            2 * pointer_size + 24
        );
        assert_eq!(
            RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET,
            2 * pointer_size + 32
        );
        assert_eq!(RESERVED_GUEST_FAULT_EXIT_V1, 6);
        assert_eq!(
            RuntimeExitTag::try_from(RESERVED_GUEST_FAULT_EXIT_V1),
            Err(6)
        );
        assert_eq!(
            RuntimeExitTag::try_from(RUNTIME_EXIT_V2_MEMORY_OPERATION),
            Err(9)
        );
        assert_eq!(std::mem::offset_of!(RuntimeContextV2, base), 0);
        assert_eq!(
            RUNTIME_CONTEXT_V2_MEMORY_ADAPTER_OFFSET,
            std::mem::size_of::<RuntimeContextV1>()
        );
        assert_eq!(
            RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET,
            std::mem::offset_of!(RuntimeContextV2, staged_load_value)
        );
        assert!(
            RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET
                > RUNTIME_CONTEXT_V2_LAST_MEMORY_STATUS_OFFSET
        );
        assert_eq!(
            RUNTIME_CONTEXT_V2_MEMORY_OUTCOME_OFFSET,
            std::mem::offset_of!(RuntimeContextV2, memory_outcome)
        );
    }
}
