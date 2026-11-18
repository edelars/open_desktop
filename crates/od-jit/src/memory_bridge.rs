//! Invocation-local checked memory helper transport.
//!
//! This is not a block exit ABI and does not authorize memory lowering in
//! `RuntimeContextV1`. A caller owns the adapter for one invocation and passes
//! only its opaque address and fixed-width values through the helper ABI.

use std::ffi::c_void;

use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

pub const MEMORY_HELPER_SUCCESS: u32 = 0;
pub const MEMORY_HELPER_ARCHITECTURAL_FAULT: u32 = 1;
pub const MEMORY_HELPER_MEMORY_ERROR: u32 = 2;
pub const MEMORY_HELPER_INVALID_ARGUMENT: u32 = 3;

pub const MEMORY_OP_FETCH: u32 = 1;
pub const MEMORY_OP_READ: u32 = 2;
pub const MEMORY_OP_WRITE: u32 = 3;

pub const MEMORY_ERROR_NONE: u32 = 0;
pub const MEMORY_ERROR_UNMAPPED: u32 = 1;
pub const MEMORY_ERROR_PERMISSION_DENIED: u32 = 2;
pub const MEMORY_ERROR_ADDRESS_OVERFLOW: u32 = 3;
pub const MEMORY_ERROR_BACKEND: u32 = 4;

/// Fixed-width output shared with generated ARM64 code. `status` is in the
/// helper namespace above; `error_kind` refines status 2 without exposing a
/// Rust enum or backend-owned string across the ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CheckedMemoryOutcomeV1 {
    pub status: u32,
    pub error_kind: u32,
    pub fault_vector: u32,
    pub has_error_code: u32,
    pub error_code: u32,
    pub has_linear_address: u32,
    pub linear_address: u64,
    pub error_address: u64,
    pub error_length: u64,
}

/// `extern "C"` entry used by the separate native probe and future generated
/// code. The adapter and all pointed-to buffers are invocation-local.
pub type CheckedMemoryHelperV1 =
    unsafe extern "C" fn(*mut c_void, u32, u64, u32, *mut u8, *mut CheckedMemoryOutcomeV1) -> u32;

/// Rust-owned bridge to one already selected guest-memory backend. The trait
/// object stays inside Rust; generated code sees only an opaque pointer.
pub struct CheckedMemoryAdapter<'a> {
    memory: &'a mut dyn GuestMemory,
    context: AccessContext,
}

impl<'a> CheckedMemoryAdapter<'a> {
    pub fn new(memory: &'a mut dyn GuestMemory, context: AccessContext) -> Self {
        Self { memory, context }
    }

    /// Opaque pointer for a synchronous helper call. It must not outlive this
    /// adapter or be retained by generated/host code.
    pub fn as_opaque_ptr(&mut self) -> *mut c_void {
        (self as *mut Self).cast()
    }
}

/// Checked helper with a fixed-width C ABI.
///
/// # Safety
/// `adapter` must point to a live `CheckedMemoryAdapter`, `data` must point to
/// `width` readable bytes for writes or writable bytes for fetch/read, and
/// `outcome` must point to one aligned writable `CheckedMemoryOutcomeV1`.
/// `adapter`, `data`, and `outcome` must not overlap. Pointers are used only
/// for this synchronous call.
pub unsafe extern "C" fn checked_memory_helper_v1(
    adapter: *mut c_void,
    operation: u32,
    address: u64,
    width: u32,
    data: *mut u8,
    outcome: *mut CheckedMemoryOutcomeV1,
) -> u32 {
    if adapter.is_null() || data.is_null() || outcome.is_null() {
        return MEMORY_HELPER_INVALID_ARGUMENT;
    }
    let Some(length) = supported_width(operation, width) else {
        return MEMORY_HELPER_INVALID_ARGUMENT;
    };

    // SAFETY: upheld by this function's caller contract.
    let adapter = unsafe { &mut *adapter.cast::<CheckedMemoryAdapter<'_>>() };
    // SAFETY: upheld by this function's caller contract.
    let outcome = unsafe { &mut *outcome };
    *outcome = CheckedMemoryOutcomeV1::default();

    let result = match operation {
        MEMORY_OP_FETCH => {
            // SAFETY: fetch requires one writable destination byte per the
            // operation-specific caller contract and width validation.
            let destination = unsafe { std::slice::from_raw_parts_mut(data, 1) };
            adapter
                .memory
                .fetch(address, adapter.context)
                .map(|byte| destination[0] = byte)
        }
        MEMORY_OP_READ => {
            // SAFETY: read requires a writable destination per the
            // operation-specific caller contract and width validation.
            let destination = unsafe { std::slice::from_raw_parts_mut(data, length) };
            let mut staged = [0_u8; 8];
            adapter
                .memory
                .read(address, adapter.context, &mut staged[..length])
                .map(|()| destination.copy_from_slice(&staged[..length]))
        }
        MEMORY_OP_WRITE => {
            // SAFETY: write requires only readable source bytes; do not create
            // a mutable slice from the ABI pointer on this path.
            let source = unsafe { std::slice::from_raw_parts(data.cast_const(), length) };
            adapter.memory.write(address, adapter.context, source)
        }
        _ => return MEMORY_HELPER_INVALID_ARGUMENT,
    };

    match result {
        Ok(()) => {
            outcome.status = MEMORY_HELPER_SUCCESS;
            MEMORY_HELPER_SUCCESS
        }
        Err(error) => {
            classify_error(error, outcome);
            outcome.status
        }
    }
}

fn supported_width(operation: u32, width: u32) -> Option<usize> {
    let width = usize::try_from(width).ok()?;
    match operation {
        MEMORY_OP_FETCH if width == 1 => Some(width),
        MEMORY_OP_READ | MEMORY_OP_WRITE if matches!(width, 1 | 2 | 4 | 8) => Some(width),
        _ => None,
    }
}

pub fn classify_error(error: MemoryError, outcome: &mut CheckedMemoryOutcomeV1) {
    match error {
        MemoryError::Architectural(fault) => {
            outcome.status = MEMORY_HELPER_ARCHITECTURAL_FAULT;
            outcome.fault_vector = u32::from(fault.vector);
            outcome.has_error_code = u32::from(fault.error_code.is_some());
            outcome.error_code = fault.error_code.unwrap_or_default();
            outcome.has_linear_address = u32::from(fault.linear_address.is_some());
            outcome.linear_address = fault.linear_address.unwrap_or_default();
        }
        MemoryError::Unmapped { address, access } => {
            outcome.status = MEMORY_HELPER_MEMORY_ERROR;
            outcome.error_kind = match access {
                AccessKind::Fetch | AccessKind::Read | AccessKind::Write => MEMORY_ERROR_UNMAPPED,
            };
            outcome.error_address = address;
        }
        MemoryError::PermissionDenied { address, access } => {
            outcome.status = MEMORY_HELPER_MEMORY_ERROR;
            outcome.error_kind = match access {
                AccessKind::Fetch | AccessKind::Read | AccessKind::Write => {
                    MEMORY_ERROR_PERMISSION_DENIED
                }
            };
            outcome.error_address = address;
        }
        MemoryError::AddressOverflow { address, length } => {
            outcome.status = MEMORY_HELPER_MEMORY_ERROR;
            outcome.error_kind = MEMORY_ERROR_ADDRESS_OVERFLOW;
            outcome.error_address = address;
            outcome.error_length = u64::try_from(length).unwrap_or(u64::MAX);
        }
        MemoryError::Backend(_) => {
            outcome.status = MEMORY_HELPER_MEMORY_ERROR;
            outcome.error_kind = MEMORY_ERROR_BACKEND;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_isa::memory::{ArchitecturalFault, GuestMemory};

    #[derive(Default)]
    struct ProbeMemory {
        bytes: [u8; 32],
        reads: usize,
        writes: usize,
        failure: Option<MemoryError>,
    }

    impl GuestMemory for ProbeMemory {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.reads += 1;
            if let Some(error) = self.failure.clone() {
                return Err(error);
            }
            self.bytes
                .get(usize::try_from(address).unwrap())
                .copied()
                .ok_or(MemoryError::Unmapped {
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
            self.reads += 1;
            if let Some(error) = self.failure.clone() {
                return Err(error);
            }
            let start = usize::try_from(address).unwrap();
            let source =
                self.bytes
                    .get(start..start + destination.len())
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Read,
                    })?;
            destination.copy_from_slice(source);
            Ok(())
        }

        fn write(
            &mut self,
            address: u64,
            _: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            self.writes += 1;
            if let Some(error) = self.failure.clone() {
                return Err(error);
            }
            let start = usize::try_from(address).unwrap();
            let destination =
                self.bytes
                    .get_mut(start..start + source.len())
                    .ok_or(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Write,
                    })?;
            destination.copy_from_slice(source);
            Ok(())
        }

        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!("not in the checked helper profile")
        }

        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!("not in the checked helper profile")
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!("not in the checked helper profile")
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn invoke(
        memory: &mut ProbeMemory,
        op: u32,
        address: u64,
        data: &mut [u8],
        outcome: &mut CheckedMemoryOutcomeV1,
    ) -> u32 {
        invoke_raw(
            memory,
            op,
            address,
            u32::try_from(data.len()).unwrap(),
            data.as_mut_ptr(),
            outcome,
        )
    }

    fn invoke_raw(
        memory: &mut ProbeMemory,
        op: u32,
        address: u64,
        width: u32,
        data: *mut u8,
        outcome: &mut CheckedMemoryOutcomeV1,
    ) -> u32 {
        let mut adapter = CheckedMemoryAdapter::new(memory, AccessContext::default());
        // SAFETY: all buffers and adapter are live for this synchronous call.
        unsafe {
            checked_memory_helper_v1(adapter.as_opaque_ptr(), op, address, width, data, outcome)
        }
    }

    #[test]
    fn supported_widths_and_fetch_are_narrow() {
        assert_eq!(supported_width(MEMORY_OP_FETCH, 1), Some(1));
        assert_eq!(supported_width(MEMORY_OP_FETCH, 2), None);
        for width in [1, 2, 4, 8] {
            assert_eq!(supported_width(MEMORY_OP_READ, width), Some(width as usize));
            assert_eq!(
                supported_width(MEMORY_OP_WRITE, width),
                Some(width as usize)
            );
        }
        assert_eq!(supported_width(MEMORY_OP_READ, 16), None);
    }

    #[test]
    fn read_stages_output_and_error_categories_without_architectural_commit() {
        let mut memory = ProbeMemory::default();
        memory.bytes[4..8].copy_from_slice(&[1, 2, 3, 4]);
        let mut data = [0xAA; 4];
        let mut outcome = CheckedMemoryOutcomeV1::default();
        assert_eq!(
            invoke(&mut memory, MEMORY_OP_READ, 4, &mut data, &mut outcome),
            0
        );
        assert_eq!(data, [1, 2, 3, 4]);

        memory.failure = Some(MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(0x1234),
        }));
        data = [0xAA; 4];
        assert_eq!(
            invoke(&mut memory, MEMORY_OP_READ, 4, &mut data, &mut outcome),
            1
        );
        assert_eq!(data, [0xAA; 4]);
        assert_eq!(outcome.fault_vector, 14);
        assert_eq!(outcome.error_code, 5);
        assert_eq!(outcome.linear_address, 0x1234);

        memory.failure = Some(MemoryError::Backend("private detail".into()));
        assert_eq!(
            invoke(&mut memory, MEMORY_OP_READ, 4, &mut data, &mut outcome),
            2
        );
        assert_eq!(outcome.error_kind, MEMORY_ERROR_BACKEND);
        assert_eq!(data, [0xAA; 4]);
    }

    #[test]
    fn write_calls_backend_once_and_rejects_unsupported_before_access() {
        let mut memory = ProbeMemory::default();
        let bytes = [7_u8, 8];
        let mut outcome = CheckedMemoryOutcomeV1::default();
        // The ABI pointer is mutable in shape for the shared read/write ABI,
        // but WRITE must accept immutable storage without creating `&mut`.
        let source = bytes.as_ptr() as *mut u8;
        assert_eq!(
            invoke_raw(&mut memory, MEMORY_OP_WRITE, 3, 2, source, &mut outcome),
            0
        );
        assert_eq!(memory.writes, 1);
        assert_eq!(&memory.bytes[3..5], &[7, 8]);

        assert_eq!(
            invoke(&mut memory, MEMORY_OP_READ, 3, &mut [0; 3], &mut outcome),
            3
        );
        assert_eq!(memory.reads, 0);
    }

    #[test]
    fn boundary_failures_are_memory_errors_separate_from_architectural_faults() {
        let mut memory = ProbeMemory::default();
        let mut data = [0xCC; 2];
        let mut outcome = CheckedMemoryOutcomeV1::default();
        assert_eq!(
            invoke(&mut memory, MEMORY_OP_READ, 31, &mut data, &mut outcome),
            MEMORY_HELPER_MEMORY_ERROR
        );
        assert_eq!(outcome.error_kind, MEMORY_ERROR_UNMAPPED);
        assert_eq!(data, [0xCC; 2]);

        memory.failure = Some(MemoryError::AddressOverflow {
            address: u64::MAX,
            length: 2,
        });
        assert_eq!(
            invoke(&mut memory, MEMORY_OP_READ, 0, &mut data, &mut outcome),
            MEMORY_HELPER_MEMORY_ERROR
        );
        assert_eq!(outcome.error_kind, MEMORY_ERROR_ADDRESS_OVERFLOW);
        assert_eq!(outcome.error_address, u64::MAX);
        assert_eq!(outcome.error_length, 2);
    }
}
