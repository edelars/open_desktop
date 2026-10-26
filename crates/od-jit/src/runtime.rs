//! Raw runtime boundary shared by future block entries and their host publisher.
//!
//! These tags are integer ABI values. `LongModeState` remains an opaque pointer:
//! generated code may use the offsets below only when compiled with this crate.

use od_isa::LongModeState;

pub const LONG_MODE_GPR_OFFSET: usize = std::mem::offset_of!(LongModeState, gpr);
pub const LONG_MODE_RIP_OFFSET: usize = std::mem::offset_of!(LongModeState, rip);
pub const LONG_MODE_RFLAGS_OFFSET: usize = std::mem::offset_of!(LongModeState, rflags);

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

/// Block entry takes a non-null `RuntimeContextV1` pointer and returns a raw tag.
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

/// Scalar helper ABI alias. Its implementation and semantics are introduced
/// with the lowering that first calls it.
pub type ArithmeticFlagsHelper = unsafe extern "C" fn(u32, u64, u64, u64, *mut u64) -> u32;

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
    }
}
