//! Raw runtime boundary shared by future block entries and their host publisher.
//!
//! These tags are integer ABI values. `LongModeState` remains an opaque pointer:
//! generated code may use the offsets below only when compiled with this crate.

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
        assert_eq!(RuntimeExitTag::try_from(8), Err(8));
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
    }
}
