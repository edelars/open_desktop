//! CPU memory boundary. Translation, paging and physical devices belong to the machine.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessKind {
    Fetch,
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivilegeLevel {
    Ring0,
    Ring1,
    Ring2,
    Ring3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionMode {
    Long64,
}

/// Describes the architectural source of a memory access independently of CPL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessOrigin {
    /// An explicit instruction fetch or data operand access.
    Explicit,
    /// An implicit read of a segment descriptor table entry.
    DescriptorTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessContext {
    pub mode: InstructionMode,
    /// Current architectural privilege level; used by instruction semantics.
    pub privilege: PrivilegeLevel,
    /// Origin can alter paging privilege without changing architectural CPL.
    pub origin: AccessOrigin,
}

impl AccessContext {
    /// Page-protection privilege for this individual linear-memory access.
    pub const fn page_privilege(self) -> PrivilegeLevel {
        match self.origin {
            AccessOrigin::Explicit => self.privilege,
            AccessOrigin::DescriptorTable => PrivilegeLevel::Ring0,
        }
    }

    pub const fn descriptor_table(self) -> Self {
        Self {
            origin: AccessOrigin::DescriptorTable,
            ..self
        }
    }
}

impl Default for AccessContext {
    fn default() -> Self {
        Self {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring3,
            origin: AccessOrigin::Explicit,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchitecturalFault {
    pub vector: u8,
    /// None means this exception has no architectural error code.
    pub error_code: Option<u32>,
    /// Supplied by the translation backend where architecturally meaningful.
    pub linear_address: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryError {
    Architectural(ArchitecturalFault),
    /// Bounded fixture mapping failure; it does not imply an x86 page fault.
    Unmapped {
        address: u64,
        access: AccessKind,
    },
    PermissionDenied {
        address: u64,
        access: AccessKind,
    },
    AddressOverflow {
        address: u64,
        length: usize,
    },
    /// Host mapping/ABI/backend failures must remain distinct from guest faults.
    Backend(String),
}

/// Addresses are guest linear u64 values; slice lengths specify the access width.
/// `AccessContext::privilege` is CPL, while page permission checks must use
/// `AccessContext::page_privilege()` (descriptor-table reads are supervisor
/// accesses even when the instruction executes at CPL3).
/// Fetch reads one byte on demand. No method assumes a contiguous host page.
///
/// Reads/writes must either complete the entire supplied access or return an error
/// without changing destination bytes or guest RAM. CPU instructions with partial
/// architectural progress (such as REP) issue separate committed accesses.
/// An MMIO backend must define supported access widths and side-effect ordering
/// before integration; this contract does not authorize retrying a device access.
pub trait GuestMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError>;
    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError>;
    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError>;

    /// Atomically exchanges one integer operand with guest memory.
    ///
    /// Implementations must perform this as one backend operation, return the
    /// previous value, and leave memory unchanged on every error. They must not
    /// synthesize the operation from separately observable `read` and `write`
    /// calls. This contract is limited to serialization provided by the
    /// implementing backend; it does not by itself define SMP memory ordering.
    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError>;

    /// Atomically adds one integer operand to guest memory and returns the old value.
    ///
    /// Implementations must perform this as one backend operation, wrap the
    /// stored result to `size`, and leave memory unchanged on every error. They
    /// must not synthesize the operation from separately observable `read` and
    /// `write` calls. The returned value is zero-extended for widths below
    /// qword. This contract is limited to serialization provided by the
    /// implementing backend; it does not imply an x86 LOCK prefix or define
    /// SMP memory ordering. The S2-21f LOCK XADD form may use this operation
    /// only with private synchronous RAM and serial interpreter access, with
    /// no external observer/writer, DMA, shared alias, delayed write, or MMIO.
    /// This is an integration precondition, not a capability checked by this
    /// trait method.
    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError>;

    /// Atomically compares a guest integer with `expected` and conditionally replaces it.
    ///
    /// Implementations return the observed old value, compare against
    /// `expected & size.mask()`, and store `replacement & size.mask()` whenever
    /// they match (no spurious mismatch). A mismatch still requires
    /// write-access validation because this instruction form has a memory
    /// read-modify-write destination. The whole operation is one backend call;
    /// every error leaves guest RAM unchanged. This method does not define x86
    /// LOCK or SMP memory ordering.
    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError>;

    /// Completes preceding guest loads/stores before subsequent guest loads/stores begin.
    ///
    /// `Ok(())` promises that earlier reads have returned their values and
    /// earlier writes/read-modify-writes are committed to canonical guest RAM.
    /// A backend may opt in only when its memory ownership and execution model
    /// can provide this completion boundary. In the S2-21g profile this is
    /// private synchronous ordinary RAM accessed serially, with no external
    /// observers/writers, DMA, shared aliases, outstanding operations, delayed
    /// writes or MMIO. On error, the current fence is not retired; previously
    /// completed instruction effects are not rolled back. This method does not
    /// modify guest RAM or CPU state, authorize MMIO, define instruction-stream
    /// serialization, or prove SMP ordering by itself.
    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError>;

    fn read_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
    ) -> Result<u64, MemoryError> {
        let mut bytes = [0u8; 8];
        self.read(address, context, &mut bytes[..size.bytes()])?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        value: u64,
    ) -> Result<(), MemoryError> {
        self.write(address, context, &value.to_le_bytes()[..size.bytes()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_table_origin_changes_page_privilege_without_changing_cpl() {
        let explicit = AccessContext::default();
        assert_eq!(explicit.privilege, PrivilegeLevel::Ring3);
        assert_eq!(explicit.page_privilege(), PrivilegeLevel::Ring3);

        let descriptor = explicit.descriptor_table();
        assert_eq!(descriptor.privilege, PrivilegeLevel::Ring3);
        assert_eq!(descriptor.origin, AccessOrigin::DescriptorTable);
        assert_eq!(descriptor.page_privilege(), PrivilegeLevel::Ring0);
    }
}
