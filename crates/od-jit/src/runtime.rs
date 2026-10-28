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

/// Maximum exact source width for a checked singleton memory MOV artifact.
/// Pair and active-code artifacts have separate, intentionally unchanged bounds.
pub const MEMORY_SITE_SOURCE_BYTES_MAX: usize = 9;

/// The checked memory operation encoded by a V2 site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemorySiteOperationV2 {
    Load,
    Store,
}

/// Exact admitted order for the bounded two-site memory artifacts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryPairSequenceV2 {
    StoreThenLoad,
    LoadThenStore,
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

fn is_address32_rbp_indexed_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    matches!(length, 5 | 6 | 9)
        && operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rax
                    | od_isa::Register::Rcx
                    | od_isa::Register::Rdx
                    | od_isa::Register::Rbx
            )
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rsi_rdi_base_rbp_index_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    matches!(length, 5 | 6 | 9)
        && operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rsp_base_rbp_indexed_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    matches!(length, 5 | 6 | 9)
        && operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rbp_base_rbp_indexed_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    matches!(length, 6 | 9)
        && operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match length {
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rbp_base_rbp_index_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    matches!(length, 5 | 8)
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rsi_rdi_base_rbp_index_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    matches!(length, 4 | 5 | 8)
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_low_base_rsi_rdi_index_sib_profile(operand: MemoryOperand, length: u64) -> bool {
    length == 4
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && operand.displacement == 0
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

    /// Validate the bounded decoded metadata profile shared by V2 compilation,
    /// host admission, and the runtime helper. Displacement semantics remain
    /// those of the decoder; this checks only the selected signed-width range.
    pub fn has_supported_profile(self) -> bool {
        self.has_supported_profile_for_index(0)
    }

    pub(crate) fn has_supported_profile_for_index(self, expected_index: u32) -> bool {
        let length = match self.next_rip.checked_sub(self.guest_rip) {
            Some(length) => length,
            None => return false,
        };
        let data_register = match self.operation {
            MemorySiteOperationV2::Load => self.destination,
            MemorySiteOperationV2::Store => self.source,
        };
        let base_is_low_gpr = matches!(
            self.operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rax
                    | od_isa::Register::Rcx
                    | od_isa::Register::Rdx
                    | od_isa::Register::Rbx
            )
        );
        let rsi_rdi_direct_data_profile = self.operand.address_size == od_isa::OperandSize::Qword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(length, 3 | 4 | 7)
            && matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(
                    od_isa::Register::Rsi | od_isa::Register::Rdi
                )
            )
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && match length {
                3 => self.operand.displacement == 0,
                4 => i8::try_from(self.operand.displacement).is_ok(),
                7 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let address32_rsi_rdi_direct_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(length, 4 | 5 | 8)
            && matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(
                    od_isa::Register::Rsi | od_isa::Register::Rdi
                )
            )
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && match length {
                4 => self.operand.displacement == 0,
                5 => i8::try_from(self.operand.displacement).is_ok(),
                8 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let indexed_sib_base_profile = base_is_low_gpr
            || matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
            )
            || matches!(length, 4 | 5 | 8)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(
                        od_isa::Register::Rsi | od_isa::Register::Rdi
                    )
                )
            || matches!(length, 5 | 8)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                );
        let indexed_sib_data_profile = matches!(length, 4 | 5 | 8)
            && indexed_sib_base_profile
            && self
                .operand
                .index
                .is_some_and(|register| (register as u8) < 4)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && match length {
                4 => self.operand.displacement == 0,
                5 => i8::try_from(self.operand.displacement).is_ok(),
                8 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let rbp_index_sib_data_profile = matches!(length, 4 | 5 | 8)
            && self.operand.address_size == od_isa::OperandSize::Qword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
            )
            && self.operand.index == Some(od_isa::Register::Rbp)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && match length {
                4 => self.operand.displacement == 0,
                5 => i8::try_from(self.operand.displacement).is_ok(),
                8 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let rsp_base_rbp_index_sib_data_profile = matches!(length, 4 | 5 | 8)
            && self.operand.address_size == od_isa::OperandSize::Qword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
            )
            && self.operand.index == Some(od_isa::Register::Rbp)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && match length {
                4 => self.operand.displacement == 0,
                5 => i8::try_from(self.operand.displacement).is_ok(),
                8 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let rbp_base_rbp_index_sib_data_profile =
            is_rbp_base_rbp_index_sib_profile(self.operand, length);
        let rsi_rdi_base_rbp_index_sib_data_profile =
            is_rsi_rdi_base_rbp_index_sib_profile(self.operand, length);
        let low_base_rsi_rdi_index_sib_data_profile =
            is_low_base_rsi_rdi_index_sib_profile(self.operand, length);
        let address32_rbp_index_sib_data_profile =
            is_address32_rbp_indexed_sib_profile(self.operand, length);
        let address32_direct_data_profile = self.operand.address_size == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && base_is_low_gpr
            && match length {
                4 => self.operand.displacement == 0,
                5 => i8::try_from(self.operand.displacement).is_ok(),
                8 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let address32_indexed_sib_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && (base_is_low_gpr
                || matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                )
                || matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(
                        od_isa::Register::Rsi | od_isa::Register::Rdi
                    )
                )
                || matches!(length, 6 | 9)
                    && matches!(
                        self.operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                    ))
            && self
                .operand
                .index
                .is_some_and(|register| (register as u8) < 4)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && match length {
                5 => self.operand.displacement == 0,
                6 => i8::try_from(self.operand.displacement).is_ok(),
                9 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let address32_rbp_indexed_sib_data_profile =
            is_address32_rbp_indexed_sib_profile(self.operand, length);
        let address32_rsi_rdi_base_rbp_index_sib_data_profile =
            is_address32_rsi_rdi_base_rbp_index_sib_profile(self.operand, length);
        let address32_rsp_base_rbp_indexed_sib_data_profile =
            is_address32_rsp_base_rbp_indexed_sib_profile(self.operand, length);
        let address32_rbp_base_rbp_indexed_sib_data_profile =
            is_address32_rbp_base_rbp_indexed_sib_profile(self.operand, length);
        let address32_rsp_sib_base_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
            )
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && match length {
                5 => self.operand.displacement == 0,
                6 => i8::try_from(self.operand.displacement).is_ok(),
                9 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let address32_rbp_direct_base_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
            )
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && match length {
                5 => i8::try_from(self.operand.displacement).is_ok(),
                8 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
        let address32_absolute_sib_disp32_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && length == 9
            && i32::try_from(self.operand.displacement).is_ok();
        let address32_indexed_no_base_sib_disp32_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
            && self.operand.index.is_some_and(|index| (index as u8) < 4)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && length == 9
            && i32::try_from(self.operand.displacement).is_ok();
        let address32_indexed_no_base_rbp_sib_disp32_data_profile = self.operand.address_size
            == od_isa::OperandSize::Dword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
            && self.operand.index == Some(od_isa::Register::Rbp)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && length == 9
            && i32::try_from(self.operand.displacement).is_ok();
        let absolute_sib_disp32_data_profile = self.operand.address_size
            == od_isa::OperandSize::Qword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
            && self.operand.index.is_none()
            && self.operand.scale == 1
            && length == 8
            && i32::try_from(self.operand.displacement).is_ok();
        let indexed_no_base_sib_disp32_data_profile = self.operand.address_size
            == od_isa::OperandSize::Qword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
            && self.operand.index.is_some_and(|index| (index as u8) < 4)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && length == 8
            && i32::try_from(self.operand.displacement).is_ok();
        let indexed_no_base_rbp_sib_disp32_data_profile = self.operand.address_size
            == od_isa::OperandSize::Qword
            && self.operand.size == od_isa::OperandSize::Qword
            && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
            && self.operand.index == Some(od_isa::Register::Rbp)
            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
            && length == 8
            && i32::try_from(self.operand.displacement).is_ok();
        let scoped_data_profile = address32_direct_data_profile
            || address32_indexed_sib_data_profile
            || address32_rbp_indexed_sib_data_profile
            || address32_rsi_rdi_base_rbp_index_sib_data_profile
            || address32_rsp_base_rbp_indexed_sib_data_profile
            || address32_rbp_base_rbp_indexed_sib_data_profile
            || address32_rsp_sib_base_data_profile
            || address32_rbp_direct_base_data_profile
            || address32_absolute_sib_disp32_data_profile
            || address32_indexed_no_base_sib_disp32_data_profile
            || address32_indexed_no_base_rbp_sib_disp32_data_profile
            || absolute_sib_disp32_data_profile
            || indexed_no_base_sib_disp32_data_profile
            || indexed_no_base_rbp_sib_disp32_data_profile
            || rsi_rdi_direct_data_profile
            || address32_rsi_rdi_direct_data_profile
            || (self.operand.address_size == od_isa::OperandSize::Qword
                && self.operand.size == od_isa::OperandSize::Qword
                && (indexed_sib_data_profile
                    || rbp_index_sib_data_profile
                    || rsp_base_rbp_index_sib_data_profile
                    || rbp_base_rbp_index_sib_data_profile
                    || rsi_rdi_base_rbp_index_sib_data_profile
                    || low_base_rsi_rdi_index_sib_data_profile
                    || (self.operand.index.is_none()
                        && self.operand.scale == 1
                        && match data_register {
                            Some(4) => {
                                (self.operand.displacement == 0
                                    && (length == 3 && base_is_low_gpr
                                        || length == 4
                                            && matches!(
                                                self.operand.base,
                                                od_isa::long_decode::AddressBase::Register(
                                                    od_isa::Register::Rsp
                                                )
                                            )))
                                    || (length == 5
                                        && i8::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rsp
                                            )
                                        ))
                                    || (length == 4
                                        && i8::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rbp
                                            )
                                        ))
                                    || (length == 4
                                        && i8::try_from(self.operand.displacement).is_ok()
                                        && base_is_low_gpr)
                                    || (length == 8
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rsp
                                            )
                                        ))
                                    || (length == 7
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rbp
                                            )
                                        ))
                                    || (length == 7
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && base_is_low_gpr)
                                    || (length == 7
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::NextRip
                                        ))
                            }
                            Some(5) => {
                                (length == 3 && self.operand.displacement == 0 && base_is_low_gpr)
                                    || (length == 4
                                        && i8::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rbp
                                            )
                                        ))
                                    || (length == 4
                                        && i8::try_from(self.operand.displacement).is_ok()
                                        && base_is_low_gpr)
                                    || (length == 4
                                        && self.operand.displacement == 0
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rsp
                                            )
                                        ))
                                    || (length == 5
                                        && i8::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rsp
                                            )
                                        ))
                                    || (length == 7
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rbp
                                            )
                                        ))
                                    || (length == 7
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && base_is_low_gpr)
                                    || (length == 7
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::NextRip
                                        ))
                                    || (length == 8
                                        && i32::try_from(self.operand.displacement).is_ok()
                                        && matches!(
                                            self.operand.base,
                                            od_isa::long_decode::AddressBase::Register(
                                                od_isa::Register::Rsp
                                            )
                                        ))
                            }
                            _ => false,
                        })));
        let register_profile = match self.operation {
            MemorySiteOperationV2::Load => {
                self.destination.is_some_and(|register| {
                    register <= 3 || (4..=5).contains(&register) && scoped_data_profile
                }) && self.source.is_none()
            }
            MemorySiteOperationV2::Store => {
                self.source.is_some_and(|register| {
                    register <= 3 || (4..=5).contains(&register) && scoped_data_profile
                }) && self.destination.is_none()
            }
        };
        let base_is_supported = match self.operand.base {
            od_isa::long_decode::AddressBase::Register(register) => {
                matches!(
                    register,
                    od_isa::Register::Rax
                        | od_isa::Register::Rcx
                        | od_isa::Register::Rdx
                        | od_isa::Register::Rbx
                ) || rsi_rdi_direct_data_profile
                    || address32_rsi_rdi_direct_data_profile
                    || matches!(register, od_isa::Register::Rsi | od_isa::Register::Rdi)
                        && (indexed_sib_data_profile
                            || address32_indexed_sib_data_profile
                            || address32_rsi_rdi_base_rbp_index_sib_data_profile)
                    || register == od_isa::Register::Rbp && matches!(length, 4 | 7)
                    || register == od_isa::Register::Rsp
                        && matches!(length, 4 | 5 | 8)
                        && self.operand.index.is_none()
                        && self.operand.scale == 1
                        && (length == 4 && self.operand.displacement == 0
                            || length == 5 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 8 && i32::try_from(self.operand.displacement).is_ok())
                    || register == od_isa::Register::Rsp
                        && self.operand.address_size == od_isa::OperandSize::Qword
                        && (self.operand.index.is_some_and(|index| (index as u8) < 4)
                            || self.operand.index == Some(od_isa::Register::Rbp))
                        && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        && (length == 4 && self.operand.displacement == 0
                            || length == 5 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 8 && i32::try_from(self.operand.displacement).is_ok())
                    || register == od_isa::Register::Rbp
                        && self.operand.address_size == od_isa::OperandSize::Qword
                        && matches!(length, 5 | 8)
                        && self.operand.index.is_some_and(|index| (index as u8) < 4)
                        && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        && (length == 5 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 8 && i32::try_from(self.operand.displacement).is_ok())
                    || register == od_isa::Register::Rbp && rbp_base_rbp_index_sib_data_profile
                    || matches!(register, od_isa::Register::Rsi | od_isa::Register::Rdi)
                        && rsi_rdi_base_rbp_index_sib_data_profile
                    || self.operand.address_size == od_isa::OperandSize::Dword
                        && register == od_isa::Register::Rsp
                        && matches!(length, 5 | 6 | 9)
                        && self.operand.index.is_none()
                        && self.operand.scale == 1
                        && (length == 5 && self.operand.displacement == 0
                            || length == 6 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 9 && i32::try_from(self.operand.displacement).is_ok())
                    || self.operand.address_size == od_isa::OperandSize::Dword
                        && register == od_isa::Register::Rsp
                        && matches!(length, 5 | 6 | 9)
                        && self.operand.index.is_some_and(|index| (index as u8) < 4)
                        && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        && (length == 5 && self.operand.displacement == 0
                            || length == 6 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 9 && i32::try_from(self.operand.displacement).is_ok())
                    || address32_rsp_base_rbp_indexed_sib_data_profile
                    || address32_rbp_base_rbp_indexed_sib_data_profile
                    || self.operand.address_size == od_isa::OperandSize::Dword
                        && register == od_isa::Register::Rbp
                        && matches!(length, 6 | 9)
                        && self.operand.index.is_some_and(|index| (index as u8) < 4)
                        && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        && (length == 6 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 9 && i32::try_from(self.operand.displacement).is_ok())
                    || self.operand.address_size == od_isa::OperandSize::Dword
                        && register == od_isa::Register::Rbp
                        && matches!(length, 5 | 8)
                        && self.operand.index.is_none()
                        && self.operand.scale == 1
                        && (length == 5 && i8::try_from(self.operand.displacement).is_ok()
                            || length == 8 && i32::try_from(self.operand.displacement).is_ok())
            }
            od_isa::long_decode::AddressBase::NextRip => {
                self.operand.index.is_none() && length == 7
            }
            od_isa::long_decode::AddressBase::None => {
                (self.operand.address_size == od_isa::OperandSize::Qword && length == 8
                    || self.operand.address_size == od_isa::OperandSize::Dword && length == 9)
                    && (self.operand.index.is_none() && self.operand.scale == 1
                        || self.operand.address_size == od_isa::OperandSize::Qword
                            && self.operand.index.is_some_and(|index| (index as u8) < 4)
                            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        || self.operand.address_size == od_isa::OperandSize::Qword
                            && length == 8
                            && self.operand.index == Some(od_isa::Register::Rbp)
                            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        || self.operand.address_size == od_isa::OperandSize::Dword
                            && self.operand.index.is_some_and(|index| (index as u8) < 4)
                            && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                        || self.operand.address_size == od_isa::OperandSize::Dword
                            && length == 9
                            && self.operand.index == Some(od_isa::Register::Rbp)
                            && matches!(self.operand.scale, 1 | 2 | 4 | 8))
                    && i32::try_from(self.operand.displacement).is_ok()
            }
        };
        if self.instruction_index != expected_index
            || !register_profile
            || !base_is_supported
            || self.operand.index.is_some_and(|index| {
                !matches!(
                    index,
                    od_isa::Register::Rax
                        | od_isa::Register::Rcx
                        | od_isa::Register::Rdx
                        | od_isa::Register::Rbx
                ) && !rbp_index_sib_data_profile
                    && !rsp_base_rbp_index_sib_data_profile
                    && !rbp_base_rbp_index_sib_data_profile
                    && !rsi_rdi_base_rbp_index_sib_data_profile
                    && !low_base_rsi_rdi_index_sib_data_profile
                    && !address32_rbp_index_sib_data_profile
                    && !address32_rsi_rdi_base_rbp_index_sib_data_profile
                    && !address32_rsp_base_rbp_indexed_sib_data_profile
                    && !address32_rbp_base_rbp_indexed_sib_data_profile
                    && !indexed_no_base_rbp_sib_disp32_data_profile
                    && !address32_indexed_no_base_rbp_sib_disp32_data_profile
            })
            || !matches!(
                self.operand.address_size,
                od_isa::OperandSize::Qword | od_isa::OperandSize::Dword
            )
            || self.operand.size != od_isa::OperandSize::Qword
        {
            return false;
        }

        if self.operand.address_size == od_isa::OperandSize::Dword {
            let supported_base = matches!(
                self.operand.base,
                od_isa::long_decode::AddressBase::Register(
                    od_isa::Register::Rax
                        | od_isa::Register::Rcx
                        | od_isa::Register::Rdx
                        | od_isa::Register::Rbx
                )
            ) || address32_rsi_rdi_direct_data_profile
                || address32_indexed_sib_data_profile
                || address32_rsi_rdi_base_rbp_index_sib_data_profile;
            let direct = self.operand.index.is_none()
                && self.operand.scale == 1
                && match length {
                    4 => self.operand.displacement == 0,
                    5 => i8::try_from(self.operand.displacement).is_ok(),
                    8 => i32::try_from(self.operand.displacement).is_ok(),
                    _ => false,
                };
            let indexed = matches!(length, 5 | 6 | 9)
                && match length {
                    5 => self.operand.displacement == 0,
                    6 => i8::try_from(self.operand.displacement).is_ok(),
                    9 => i32::try_from(self.operand.displacement).is_ok(),
                    _ => false,
                }
                && self.operand.index.is_some_and(|index| {
                    matches!(
                        index,
                        od_isa::Register::Rax
                            | od_isa::Register::Rcx
                            | od_isa::Register::Rdx
                            | od_isa::Register::Rbx
                    ) || index == od_isa::Register::Rbp
                })
                && matches!(self.operand.scale, 1 | 2 | 4 | 8);
            let rsp_no_index_sib = matches!(length, 5 | 6 | 9)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                )
                && self.operand.index.is_none()
                && self.operand.scale == 1
                && (length == 5 && self.operand.displacement == 0
                    || length == 6 && i8::try_from(self.operand.displacement).is_ok()
                    || length == 9 && i32::try_from(self.operand.displacement).is_ok());
            let rsp_indexed_sib = matches!(length, 5 | 6 | 9)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                )
                && self.operand.index.is_some_and(|index| {
                    matches!(
                        index,
                        od_isa::Register::Rax
                            | od_isa::Register::Rcx
                            | od_isa::Register::Rdx
                            | od_isa::Register::Rbx
                    )
                })
                && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                && (length == 5 && self.operand.displacement == 0
                    || length == 6 && i8::try_from(self.operand.displacement).is_ok()
                    || length == 9 && i32::try_from(self.operand.displacement).is_ok());
            let rsp_base_rbp_indexed_sib =
                is_address32_rsp_base_rbp_indexed_sib_profile(self.operand, length);
            let rbp_indexed_sib = matches!(length, 6 | 9)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )
                && self.operand.index.is_some_and(|index| (index as u8) < 4)
                && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                && (length == 6 && i8::try_from(self.operand.displacement).is_ok()
                    || length == 9 && i32::try_from(self.operand.displacement).is_ok());
            let rbp_base_rbp_indexed_sib =
                is_address32_rbp_base_rbp_indexed_sib_profile(self.operand, length);
            let rbp_direct = matches!(length, 5 | 8)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )
                && self.operand.index.is_none()
                && self.operand.scale == 1
                && (length == 5 && i8::try_from(self.operand.displacement).is_ok()
                    || length == 8 && i32::try_from(self.operand.displacement).is_ok());
            let absolute_sib_disp32 = length == 9
                && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                && self.operand.index.is_none()
                && self.operand.scale == 1
                && i32::try_from(self.operand.displacement).is_ok();
            let indexed_no_base_sib_disp32 = length == 9
                && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                && self.operand.index.is_some_and(|index| (index as u8) < 4)
                && matches!(self.operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(self.operand.displacement).is_ok();
            let indexed_no_base_rbp_sib_disp32 =
                address32_indexed_no_base_rbp_sib_disp32_data_profile;
            return (supported_base
                || rsp_no_index_sib
                || rsp_indexed_sib
                || rsp_base_rbp_indexed_sib
                || rbp_indexed_sib
                || rbp_base_rbp_indexed_sib
                || rbp_direct
                || absolute_sib_disp32
                || indexed_no_base_sib_disp32
                || indexed_no_base_rbp_sib_disp32)
                && (direct
                    || indexed
                    || address32_indexed_sib_data_profile
                    || rsp_no_index_sib
                    || rsp_indexed_sib
                    || rsp_base_rbp_indexed_sib
                    || rbp_indexed_sib
                    || rbp_base_rbp_indexed_sib
                    || rbp_direct
                    || absolute_sib_disp32
                    || indexed_no_base_sib_disp32
                    || indexed_no_base_rbp_sib_disp32
                    || address32_rsi_rdi_base_rbp_index_sib_data_profile);
        }

        let displacement_is_valid = match (self.operand.base, self.operand.index, length) {
            (od_isa::long_decode::AddressBase::NextRip, None, 7) => {
                i32::try_from(self.operand.displacement).is_ok()
            }
            (od_isa::long_decode::AddressBase::Register(_), None, 3)
            | (od_isa::long_decode::AddressBase::Register(_), Some(_), 4) => {
                self.operand.displacement == 0
            }
            (od_isa::long_decode::AddressBase::Register(_), None, 4)
            | (od_isa::long_decode::AddressBase::Register(_), Some(_), 5) => {
                i8::try_from(self.operand.displacement).is_ok()
            }
            (od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp), None, 5) => {
                i8::try_from(self.operand.displacement).is_ok()
            }
            (od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp), None, 8) => {
                i32::try_from(self.operand.displacement).is_ok()
            }
            (od_isa::long_decode::AddressBase::Register(_), None, 7)
            | (od_isa::long_decode::AddressBase::Register(_), Some(_), 8) => {
                i32::try_from(self.operand.displacement).is_ok()
            }
            (od_isa::long_decode::AddressBase::None, None, 8 | 9)
            | (od_isa::long_decode::AddressBase::None, Some(_), 8) => {
                i32::try_from(self.operand.displacement).is_ok()
            }
            _ => return false,
        };
        let expected_scale = if self.operand.index.is_some() {
            matches!(self.operand.scale, 1 | 2 | 4 | 8)
        } else {
            self.operand.scale == 1
        };
        displacement_is_valid && expected_scale
    }

    /// Validate that immutable source bytes encode this decoded site in the
    /// narrow qword MOV profile. The decoder remains the only authority for
    /// displacement interpretation; raw displacement bytes are not re-decoded.
    pub fn matches_source_bytes(
        self,
        source: &[u8],
        expected_operation: MemorySiteOperationV2,
    ) -> bool {
        self.matches_source_bytes_for_index(source, expected_operation, 0)
    }

    pub fn matches_source_bytes_for_index(
        self,
        source: &[u8],
        expected_operation: MemorySiteOperationV2,
        expected_index: u32,
    ) -> bool {
        if !self.has_supported_profile_for_index(expected_index)
            || self.operation != expected_operation
        {
            return false;
        }
        let opcode = match expected_operation {
            MemorySiteOperationV2::Load => 0x8b,
            MemorySiteOperationV2::Store => 0x89,
        };
        if source.first() == Some(&0x67) {
            if source.len() < 4 || source[1] != 0x48 || source[2] != opcode {
                return false;
            }
            let modrm = source[3];
            let mode = modrm >> 6;
            let data_register = (modrm >> 3) & 7;
            let base = modrm & 7;
            let site_register = match expected_operation {
                MemorySiteOperationV2::Load => self.destination,
                MemorySiteOperationV2::Store => self.source,
            };
            if self.operand.address_size == od_isa::OperandSize::Dword
                && source.len() == 9
                && mode == 0
                && base == 4
                && source[4] == 0x25
            {
                return data_register <= 5
                    && site_register == Some(data_register)
                    && self.next_rip.checked_sub(self.guest_rip) == Some(9)
                    && self.operand.size == od_isa::OperandSize::Qword
                    && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                    && self.operand.index.is_none()
                    && self.operand.scale == 1
                    && i32::try_from(self.operand.displacement).is_ok();
            }
            if self.operand.address_size == od_isa::OperandSize::Dword
                && source.len() == 9
                && mode == 0
                && base == 4
                && source[4] & 7 == 5
                && ((source[4] >> 3) & 7 < 4 || (source[4] >> 3) & 7 == 5)
            {
                let sib = source[4];
                return data_register <= 5
                    && site_register == Some(data_register)
                    && self.next_rip.checked_sub(self.guest_rip) == Some(9)
                    && self.operand.address_size == od_isa::OperandSize::Dword
                    && self.operand.size == od_isa::OperandSize::Qword
                    && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                    && self.operand.index == od_isa::Register::from_encoding((sib >> 3) & 7)
                    && self.operand.scale == 1_u8 << (sib >> 6)
                    && i32::try_from(self.operand.displacement).is_ok();
            }
            if self.operand.address_size == od_isa::OperandSize::Dword
                && base == 4
                && matches!((source.len(), mode), (5, 0) | (6, 1) | (9, 2))
                && matches!(source[4] & 7, 6 | 7)
                && (source[4] >> 3) & 7 == 5
            {
                let sib = source[4];
                let displacement_valid = match source.len() {
                    5 => self.operand.displacement == 0,
                    6 => i8::try_from(self.operand.displacement).is_ok(),
                    9 => i32::try_from(self.operand.displacement).is_ok(),
                    _ => false,
                };
                return data_register <= 5
                    && site_register == Some(data_register)
                    && self.next_rip.checked_sub(self.guest_rip) == Some(source.len() as u64)
                    && is_address32_rsi_rdi_base_rbp_index_sib_profile(
                        self.operand,
                        source.len() as u64,
                    )
                    && matches!(
                        (source[4] & 7, self.operand.base),
                        (
                            6,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsi)
                        ) | (
                            7,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rdi)
                        )
                    )
                    && self.operand.index == Some(od_isa::Register::Rbp)
                    && self.operand.scale == 1_u8 << (sib >> 6)
                    && displacement_valid;
            }
            if self.operand.address_size == od_isa::OperandSize::Dword
                && base == 4
                && matches!((source.len(), mode), (5, 0) | (6, 1) | (9, 2))
                && source[4] & 7 < 4
                && (source[4] >> 3) & 7 == 5
            {
                let sib = source[4];
                let Some(decoded_sib_base) = od_isa::Register::from_encoding(sib & 7) else {
                    return false;
                };
                let displacement_valid = match source.len() {
                    5 => self.operand.displacement == 0,
                    6 => i8::try_from(self.operand.displacement).is_ok(),
                    9 => i32::try_from(self.operand.displacement).is_ok(),
                    _ => false,
                };
                return data_register <= 5
                    && site_register == Some(data_register)
                    && self.next_rip.checked_sub(self.guest_rip) == Some(source.len() as u64)
                    && self.operand.address_size == od_isa::OperandSize::Dword
                    && self.operand.size == od_isa::OperandSize::Qword
                    && matches!(
                        self.operand.base,
                        od_isa::long_decode::AddressBase::Register(register)
                            if register == decoded_sib_base
                    )
                    && self.operand.index == Some(od_isa::Register::Rbp)
                    && self.operand.scale == 1_u8 << (sib >> 6)
                    && displacement_valid;
            }
            let od_isa::long_decode::AddressBase::Register(decoded_base) = self.operand.base else {
                return false;
            };
            if base == 4 {
                let Some(&sib) = source.get(4) else {
                    return false;
                };
                if sib == 0x24 {
                    return matches!((source.len(), mode), (5, 0) | (6, 1) | (9, 2))
                        && data_register <= 5
                        && site_register == Some(data_register)
                        && self.next_rip.checked_sub(self.guest_rip) == Some(source.len() as u64)
                        && self.operand.address_size == od_isa::OperandSize::Dword
                        && self.operand.size == od_isa::OperandSize::Qword
                        && matches!(
                            self.operand.base,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                        )
                        && self.operand.index.is_none()
                        && self.operand.scale == 1
                        && match source.len() {
                            5 => self.operand.displacement == 0,
                            6 => i8::try_from(self.operand.displacement).is_ok(),
                            9 => i32::try_from(self.operand.displacement).is_ok(),
                            _ => false,
                        };
                }
                let sib_base = sib & 7;
                let sib_index = (sib >> 3) & 7;
                let scale = 1_u8 << (sib >> 6);
                let Some(decoded_sib_base) = od_isa::Register::from_encoding(sib_base) else {
                    return false;
                };
                let Some(decoded_sib_index) = od_isa::Register::from_encoding(sib_index) else {
                    return false;
                };
                let rsp_indexed_sib = sib_base == 4;
                let rsi_rdi_indexed_sib = matches!(sib_base, 6 | 7) && sib_index < 4;
                let rbp_indexed_sib = sib_base == 5 && matches!(mode, 1 | 2);
                let rsp_base_rbp_indexed_sib = sib_base == 4 && sib_index == 5;
                let rbp_base_rbp_indexed_sib =
                    sib_base == 5 && sib_index == 5 && matches!(mode, 1 | 2);
                return matches!((source.len(), mode), (5, 0) | (6, 1) | (9, 2))
                    && data_register <= 5
                    && (sib_base < 4
                        || rsp_indexed_sib && sib_index < 4
                        || rsi_rdi_indexed_sib
                        || rbp_indexed_sib && sib_index < 4
                        || rsp_base_rbp_indexed_sib
                        || rbp_base_rbp_indexed_sib)
                    && site_register == Some(data_register)
                    && self.next_rip.checked_sub(self.guest_rip) == Some(source.len() as u64)
                    && self.operand.address_size == od_isa::OperandSize::Dword
                    && self.operand.size == od_isa::OperandSize::Qword
                    && decoded_base == decoded_sib_base
                    && self.operand.index == Some(decoded_sib_index)
                    && self.operand.scale == scale
                    && match source.len() {
                        5 => self.operand.displacement == 0,
                        6 => i8::try_from(self.operand.displacement).is_ok(),
                        9 => i32::try_from(self.operand.displacement).is_ok(),
                        _ => false,
                    };
            }
            let (expected_length, displacement_valid) = match mode {
                0 => (4, self.operand.displacement == 0),
                1 => (5, i8::try_from(self.operand.displacement).is_ok()),
                2 => (8, i32::try_from(self.operand.displacement).is_ok()),
                _ => return false,
            };
            return source.len() == expected_length
                && data_register <= 5
                && (base < 4
                    || base == 6 && decoded_base == od_isa::Register::Rsi
                    || base == 7 && decoded_base == od_isa::Register::Rdi
                    || matches!(mode, 1 | 2)
                        && base == 5
                        && decoded_base == od_isa::Register::Rbp)
                && site_register == Some(data_register)
                && self.next_rip.checked_sub(self.guest_rip) == Some(expected_length as u64)
                && self.operand.address_size == od_isa::OperandSize::Dword
                && self.operand.size == od_isa::OperandSize::Qword
                && od_isa::Register::from_encoding(base) == Some(decoded_base)
                && self.operand.index.is_none()
                && self.operand.scale == 1
                && displacement_valid;
        }
        // Every legacy source shape below uses the original 64-bit address
        // size. Keep it explicit now that the separate 67 profile is admitted.
        if self.operand.address_size != od_isa::OperandSize::Qword {
            return false;
        }
        if source.len() < 3 || source[0] != 0x48 || source[1] != opcode {
            return false;
        }
        let modrm = source[2];
        let mode = modrm >> 6;
        let data_register = (modrm >> 3) & 7;
        let base = modrm & 7;
        if matches!((source.len(), mode), (3, 0) | (4, 1) | (7, 2)) && matches!(base, 6 | 7) {
            let site_register = match expected_operation {
                MemorySiteOperationV2::Load => self.destination,
                MemorySiteOperationV2::Store => self.source,
            };
            let displacement_valid = match source.len() {
                3 => self.operand.displacement == 0,
                4 => i8::try_from(self.operand.displacement).is_ok(),
                7 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
            return data_register <= 5
                && site_register == Some(data_register)
                && self.next_rip.checked_sub(self.guest_rip) == Some(source.len() as u64)
                && self.operand.address_size == od_isa::OperandSize::Qword
                && self.operand.size == od_isa::OperandSize::Qword
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(register)
                        if od_isa::Register::from_encoding(base) == Some(register)
                )
                && self.operand.index.is_none()
                && self.operand.scale == 1
                && displacement_valid;
        }
        if mode == 0 && modrm & 7 == 4 && source.len() == 8 && source[3] == 0x25 {
            let site_register = match expected_operation {
                MemorySiteOperationV2::Load => self.destination,
                MemorySiteOperationV2::Store => self.source,
            };
            return data_register <= 5
                && site_register == Some(data_register)
                && self.next_rip.checked_sub(self.guest_rip) == Some(8)
                && self.operand.address_size == od_isa::OperandSize::Qword
                && self.operand.size == od_isa::OperandSize::Qword
                && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                && self.operand.index.is_none()
                && self.operand.scale == 1
                && i32::try_from(self.operand.displacement).is_ok();
        }
        if mode == 0
            && modrm & 7 == 4
            && source.len() == 8
            && source[3] & 7 == 5
            && (source[3] >> 3) & 7 < 4
        {
            let site_register = match expected_operation {
                MemorySiteOperationV2::Load => self.destination,
                MemorySiteOperationV2::Store => self.source,
            };
            let sib = source[3];
            return data_register <= 5
                && site_register == Some(data_register)
                && self.next_rip.checked_sub(self.guest_rip) == Some(8)
                && self.operand.address_size == od_isa::OperandSize::Qword
                && self.operand.size == od_isa::OperandSize::Qword
                && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                && self.operand.index == od_isa::Register::from_encoding((sib >> 3) & 7)
                && self.operand.scale == 1_u8 << (sib >> 6)
                && i32::try_from(self.operand.displacement).is_ok();
        }
        if matches!(mode, 0 | 1 | 2)
            && modrm & 7 == 4
            && source
                .get(3)
                .is_some_and(|sib| matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4)
        {
            let expected_length = match mode {
                0 => 4,
                1 => 5,
                2 => 8,
                _ => unreachable!(),
            };
            if source.len() != expected_length {
                return false;
            }
            let displacement_valid = match mode {
                0 => self.operand.displacement == 0,
                1 => i8::try_from(self.operand.displacement).is_ok(),
                2 => i32::try_from(self.operand.displacement).is_ok(),
                _ => false,
            };
            let sib = source[3];
            let Some(decoded_base) = od_isa::Register::from_encoding(sib & 7) else {
                return false;
            };
            let site_register = match expected_operation {
                MemorySiteOperationV2::Load => self.destination,
                MemorySiteOperationV2::Store => self.source,
            };
            return data_register <= 5
                && site_register == Some(data_register)
                && self.next_rip.checked_sub(self.guest_rip) == Some(expected_length as u64)
                && self.operand.address_size == od_isa::OperandSize::Qword
                && self.operand.size == od_isa::OperandSize::Qword
                && matches!(decoded_base, od_isa::Register::Rsi | od_isa::Register::Rdi)
                && matches!(
                    self.operand.base,
                    od_isa::long_decode::AddressBase::Register(register)
                        if register == decoded_base
                )
                && self.operand.index == od_isa::Register::from_encoding((sib >> 3) & 7)
                && self.operand.scale == 1_u8 << (sib >> 6)
                && displacement_valid;
        }
        if mode == 0
            && modrm & 7 == 4
            && source.len() == 8
            && source[3] & 7 == 5
            && (source[3] >> 3) & 7 == 5
        {
            let site_register = match expected_operation {
                MemorySiteOperationV2::Load => self.destination,
                MemorySiteOperationV2::Store => self.source,
            };
            let sib = source[3];
            return data_register <= 5
                && site_register == Some(data_register)
                && self.next_rip.checked_sub(self.guest_rip) == Some(8)
                && self.operand.address_size == od_isa::OperandSize::Qword
                && self.operand.size == od_isa::OperandSize::Qword
                && matches!(self.operand.base, od_isa::long_decode::AddressBase::None)
                && self.operand.index == Some(od_isa::Register::Rbp)
                && self.operand.scale == 1_u8 << (sib >> 6)
                && i32::try_from(self.operand.displacement).is_ok();
        }
        if mode == 3
            || data_register > 5
            || data_register >= 4
                && !((mode == 0 && modrm & 7 < 4 && source.len() == 3)
                    || (data_register == 4 && mode == 0 && modrm == 0x24 && source.len() == 4)
                    || (data_register == 4 && mode == 1 && modrm == 0x65 && source.len() == 4)
                    || (data_register == 4 && mode == 2 && modrm == 0xa5 && source.len() == 7)
                    || (data_register == 5 && mode == 0 && modrm == 0x2c && source.len() == 4)
                    || (data_register == 4 && mode == 1 && modrm & 7 < 4 && source.len() == 4)
                    || (data_register == 5 && mode == 1 && modrm & 7 < 4 && source.len() == 4)
                    || (data_register == 4 && mode == 2 && modrm & 7 < 4 && source.len() == 7)
                    || (data_register == 5 && mode == 2 && modrm & 7 < 4 && source.len() == 7)
                    || (data_register == 4 && mode == 0 && modrm == 0x25 && source.len() == 7)
                    || (data_register == 5 && mode == 0 && modrm == 0x2d && source.len() == 7)
                    || (data_register == 4 && mode == 1 && modrm == 0x64 && source.len() == 5)
                    || (data_register == 5 && mode == 1 && modrm == 0x6c && source.len() == 5)
                    || (data_register == 4 && mode == 2 && modrm == 0xa4 && source.len() == 8)
                    || (data_register == 5 && mode == 2 && modrm == 0xac && source.len() == 8)
                    || (data_register == 5 && mode == 1 && modrm == 0x6d && source.len() == 4)
                    || (data_register == 5 && mode == 2 && modrm == 0xad && source.len() == 7)
                    || (data_register == 4
                        && mode == 0
                        && modrm == 0x24
                        && source.len() == 8
                        && source[3] & 7 == 5
                        && (source[3] >> 3) & 7 < 4)
                    || (data_register == 5
                        && mode == 0
                        && modrm == 0x2c
                        && source.len() == 8
                        && source[3] & 7 == 5
                        && (source[3] >> 3) & 7 < 4)
                    || (data_register >= 4
                        && mode == 0
                        && modrm & 7 == 4
                        && source.len() == 8
                        && source[3] & 7 == 5
                        && (source[3] >> 3) & 7 == 5)
                    || (data_register >= 4
                        && modrm & 7 == 4
                        && matches!((mode, source.len()), (0, 4) | (1, 5) | (2, 8))
                        && source[3] & 7 == 4
                        && (source[3] >> 3) & 7 == 5)
                    || (data_register >= 4
                        && mode == 0
                        && modrm & 7 == 4
                        && source.len() == 4
                        && source[3] & 7 < 4
                        && matches!((source[3] >> 3) & 7, 6 | 7)))
        {
            return false;
        }
        let rip_relative = mode == 0 && modrm & 7 == 5;
        let displacement_bytes = match (mode, rip_relative) {
            (0, true) => 4,
            (0, false) => 0,
            (1, _) => 1,
            (2, _) => 4,
            _ => return false,
        };
        let has_sib = modrm & 7 == 4;
        let expected_length = 3 + usize::from(has_sib) + displacement_bytes;
        if source.len() != expected_length
            || self.next_rip.checked_sub(self.guest_rip) != Some(expected_length as u64)
        {
            return false;
        }

        let register = match expected_operation {
            MemorySiteOperationV2::Load => self.destination,
            MemorySiteOperationV2::Store => self.source,
        };
        if register != Some((modrm >> 3) & 7) {
            return false;
        }
        if has_sib {
            let sib = source[3];
            let base = sib & 7;
            let index = (sib >> 3) & 7;
            let scale = 1_u8 << (sib >> 6);
            let od_isa::long_decode::AddressBase::Register(decoded_base) = self.operand.base else {
                return false;
            };
            if mode == 0 && sib == 0x24 {
                return source.len() == 4
                    && ((modrm >> 3) & 7 < 4 || matches!(modrm, 0x24 | 0x2c))
                    && decoded_base == od_isa::Register::Rsp
                    && self.operand.index.is_none()
                    && self.operand.scale == 1
                    && self.operand.displacement == 0;
            }
            if mode == 1 && sib == 0x24 {
                return source.len() == 5
                    && ((modrm >> 3) & 7 < 4 || matches!(modrm, 0x64 | 0x6c))
                    && decoded_base == od_isa::Register::Rsp
                    && self.operand.index.is_none()
                    && self.operand.scale == 1
                    && i8::try_from(self.operand.displacement).is_ok();
            }
            if mode == 2 && sib == 0x24 {
                return source.len() == 8
                    && ((modrm >> 3) & 7 < 4 || matches!(modrm, 0xa4 | 0xac))
                    && decoded_base == od_isa::Register::Rsp
                    && self.operand.index.is_none()
                    && self.operand.scale == 1
                    && i32::try_from(self.operand.displacement).is_ok();
            }
            let rbp_indexed_length = match (mode, source.len()) {
                (0, 4) => Some(4),
                (1, 5) => Some(5),
                (2, 8) => Some(8),
                _ => None,
            };
            if rbp_indexed_length == Some(4) && base < 4 && matches!(index, 6 | 7) {
                return data_register <= 5
                    && self.next_rip.checked_sub(self.guest_rip) == Some(4)
                    && is_low_base_rsi_rdi_index_sib_profile(self.operand, 4)
                    && od_isa::Register::from_encoding(base) == Some(decoded_base)
                    && self.operand.index == od_isa::Register::from_encoding(index)
                    && self.operand.scale == scale;
            }
            if let Some(expected_length) = rbp_indexed_length
                && (base < 4 || base == 4 && decoded_base == od_isa::Register::Rsp)
                && index == 5
            {
                return data_register <= 5
                    && self.next_rip.checked_sub(self.guest_rip) == Some(expected_length as u64)
                    && self.operand.address_size == od_isa::OperandSize::Qword
                    && self.operand.size == od_isa::OperandSize::Qword
                    && od_isa::Register::from_encoding(base) == Some(decoded_base)
                    && self.operand.index == Some(od_isa::Register::Rbp)
                    && self.operand.scale == scale
                    && match expected_length {
                        4 => self.operand.displacement == 0,
                        5 => i8::try_from(self.operand.displacement).is_ok(),
                        8 => i32::try_from(self.operand.displacement).is_ok(),
                        _ => false,
                    };
            }
            if let Some(expected_length) = rbp_indexed_length
                && matches!(base, 6 | 7)
                && index == 5
            {
                return data_register <= 5
                    && self.next_rip.checked_sub(self.guest_rip) == Some(expected_length as u64)
                    && od_isa::Register::from_encoding(base) == Some(decoded_base)
                    && is_rsi_rdi_base_rbp_index_sib_profile(self.operand, expected_length as u64)
                    && self.operand.index == Some(od_isa::Register::Rbp)
                    && self.operand.scale == scale;
            }
            if matches!((mode, source.len()), (1, 5) | (2, 8)) && base == 5 && index == 5 {
                return data_register <= 5
                    && decoded_base == od_isa::Register::Rbp
                    && self.next_rip.checked_sub(self.guest_rip) == Some(source.len() as u64)
                    && is_rbp_base_rbp_index_sib_profile(self.operand, source.len() as u64)
                    && self.operand.scale == scale;
            }
            (base < 4
                || base == 4 && decoded_base == od_isa::Register::Rsp
                || base == 5 && mode != 0 && decoded_base == od_isa::Register::Rbp)
                && index < 4
                && od_isa::Register::from_encoding(base) == Some(decoded_base)
                && self.operand.index == od_isa::Register::from_encoding(index)
                && self.operand.scale == scale
        } else {
            let base = modrm & 7;
            match (mode, base, self.operand.base) {
                (0, 5, od_isa::long_decode::AddressBase::NextRip) => {
                    self.operand.index.is_none() && self.operand.scale == 1 && source.len() == 7
                }
                (0..=2, 0..=3, od_isa::long_decode::AddressBase::Register(decoded_base)) => {
                    od_isa::Register::from_encoding(base) == Some(decoded_base)
                        && self.operand.index.is_none()
                        && self.operand.scale == 1
                }
                (1, 5, od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)) => {
                    self.operand.index.is_none()
                        && self.operand.scale == 1
                        && i8::try_from(self.operand.displacement).is_ok()
                }
                (2, 5, od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)) => {
                    self.operand.index.is_none()
                        && self.operand.scale == 1
                        && i32::try_from(self.operand.displacement).is_ok()
                }
                _ => false,
            }
        }
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

/// Invocation record for the bounded active-code S3-24a artifact. This is a
/// distinct entry ABI; its V2 memory payload is only the nested checked-store
/// transport, never an entry point accepted by the V2 APIs.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RuntimeContextV3 {
    pub memory: RuntimeContextV2,
    /// Set synchronously by the tracked owner after a committed write overlaps
    /// already executed bytes in the active artifact.
    pub code_dirty: u32,
    pub reserved: u32,
    /// Original invocation budget, retained while nested V2 code uses the
    /// remaining budget to guard the current instruction.
    pub total_initial_budget: u64,
}

pub const RUNTIME_CONTEXT_V3_CODE_DIRTY_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV3, code_dirty);
pub const RUNTIME_CONTEXT_V3_TOTAL_INITIAL_BUDGET_OFFSET: usize =
    std::mem::offset_of!(RuntimeContextV3, total_initial_budget);

/// Distinct native entry type for verifier-issued active-code artifacts.
pub type BlockEntryV3 = unsafe extern "C" fn(*mut RuntimeContextV3) -> u32;

/// Exit namespace for the S3-24a entry. Values are private to this typed V3
/// artifact and are never passed through V1 or V2 result decoders.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeExitV3 {
    Complete = 0,
    BudgetExhausted = 1,
    StopRequested = 3,
    StopAddress = 4,
    CodeInvalidated = 10,
    MemoryOperation = 9,
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

/// Trusted singleton helper used by verified V2 checked-memory blocks. It
/// retains the legacy site-index-zero contract; effective address validation
/// remains in `MemoryOperand::access_address_48` and memory access is checked.
///
/// # Safety
/// `context` must point to a live invocation-local `RuntimeContextV2` whose
/// state, adapter, and immutable memory-site table remain valid for the call.
pub unsafe extern "C" fn memory_site_helper_v2(
    context: *mut RuntimeContextV2,
    site_index: u32,
) -> u32 {
    unsafe { memory_site_helper_v2_impl(context, site_index, 0) }
}

/// Helper reserved for the two exact bounded two-instruction artifacts. Table
/// index and instruction index must both be 0 or 1; the singleton V2 helper
/// retains its original index-zero contract.
///
/// # Safety
/// `context` must point to a live, aligned, exclusively borrowed
/// `RuntimeContextV2`. Its state, adapter, and immutable two-entry site table
/// must all be valid and remain alive for the entire synchronous call. The
/// adapter must provide exclusive access to private ordinary RAM. Qword writes
/// must be all-or-nothing. Accesses must not be MMIO, atomic, delayed,
/// asynchronous, concurrent, DMA-visible, or executable/code aliases. The
/// adapter must not re-enter or unwind through this helper. Context outputs
/// must remain writable and unaliased until return.
pub unsafe extern "C" fn memory_site_helper_two_site_v2(
    context: *mut RuntimeContextV2,
    site_index: u32,
) -> u32 {
    if site_index > 1 {
        return crate::memory_bridge::MEMORY_HELPER_INVALID_ARGUMENT;
    }
    unsafe { memory_site_helper_v2_impl(context, site_index, site_index) }
}

unsafe fn memory_site_helper_v2_impl(
    context: *mut RuntimeContextV2,
    site_index: u32,
    expected_instruction_index: u32,
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
    if !site.has_supported_profile_for_index(expected_instruction_index)
        || state.rip != site.guest_rip
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
