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
    /// An implicit access made while the processor constructs a stack frame.
    ImplicitStack,
    /// An implicit access to a segment descriptor table entry, including writes.
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
            AccessOrigin::Explicit | AccessOrigin::ImplicitStack => self.privilege,
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

/// Builds the x86 page-fault error code for the supported paging profile.
/// Reserved-bit violations and protection violations set P; not-present
/// entries clear it. I/D is reported for instruction fetches when NX is active.
pub const fn page_fault_error_code(
    access: AccessKind,
    context: AccessContext,
    present: bool,
    reserved: bool,
    nxe: bool,
) -> u32 {
    (present as u32)
        | ((matches!(access, AccessKind::Write) as u32) << 1)
        | ((matches!(context.page_privilege(), PrivilegeLevel::Ring3) as u32) << 2)
        | ((reserved as u32) << 3)
        | ((matches!(access, AccessKind::Fetch) && nxe) as u32) << 4
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchitecturalFault {
    pub vector: u8,
    /// None means this exception has no architectural error code.
    pub error_code: Option<u32>,
    /// Supplied by the translation backend where architecturally meaningful.
    pub linear_address: Option<u64>,
}

/// A precisely described page fault, before CPU exception delivery.
/// `cr2_value` is the value intended for CR2; constructing this record does not
/// update CPU state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageFaultRecord {
    fault: ArchitecturalFault,
    saved_rip: u64,
    access: AccessKind,
    context: AccessContext,
}

impl PageFaultRecord {
    pub const fn new(
        saved_rip: u64,
        access: AccessKind,
        context: AccessContext,
        error_code: u32,
        linear_address: u64,
    ) -> Self {
        Self {
            fault: ArchitecturalFault {
                vector: 14,
                error_code: Some(error_code),
                linear_address: Some(linear_address),
            },
            saved_rip,
            access,
            context,
        }
    }

    pub const fn fault(&self) -> &ArchitecturalFault {
        &self.fault
    }

    pub const fn saved_rip(&self) -> u64 {
        self.saved_rip
    }

    pub const fn access(&self) -> AccessKind {
        self.access
    }

    pub const fn context(&self) -> AccessContext {
        self.context
    }

    pub const fn cr2_value(&self) -> u64 {
        // The constructor always supplies the required linear address.
        match self.fault.linear_address {
            Some(address) => address,
            None => unreachable!(),
        }
    }
}

/// A double fault caused by a page fault while delivering an earlier #PF.
/// The original RIP is retained for diagnostics only; #DF is an abort and the
/// saved RIP is not a restartable instruction address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoubleFaultRecord {
    original: PageFaultRecord,
    secondary: PageFaultRecord,
}

impl DoubleFaultRecord {
    pub const fn new(original: PageFaultRecord, secondary: PageFaultRecord) -> Self {
        Self {
            original,
            secondary,
        }
    }

    pub const fn original(&self) -> &PageFaultRecord {
        &self.original
    }

    pub const fn secondary(&self) -> &PageFaultRecord {
        &self.secondary
    }
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

/// The kind of a guest fence instruction (S6-32): `MFENCE` is `Full`, `SFENCE` is `Store`,
/// `LFENCE` is `Load`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FenceKind {
    Full,
    Store,
    Load,
}

/// Addresses are guest linear u64 values; slice lengths specify the access width.
/// `AccessContext::privilege` is CPL, while page permission checks must use
/// `AccessContext::page_privilege()` (implicit descriptor-table reads and
/// writes use supervisor page privilege even when the instruction executes at
/// CPL3).
/// Fetch reads one byte on demand. No method assumes a contiguous host page.
///
/// A successful `read` fills the entire destination buffer. An error from `read`
/// leaves that result buffer unchanged; a backend must not expose a partial read.
/// A successful `write` commits the entire supplied operand. An error from
/// `write` leaves the operand bytes without a partial operand commit. These
/// guarantees apply to the result buffer and operand transaction, not to every
/// byte in guest RAM: translation metadata writes, such as page-table A/D
/// updates, are separate architectural effects and may persist before a later
/// access fault. Such effects may emit `committed_write` notifications before
/// the fault is returned.
///
/// In particular, a page-table byte may alias the bytes of the requested
/// operand. A/D updates through that alias are allowed to change guest RAM even
/// when the later operand read or write fails; implementations must not claim a
/// full-RAM rollback. CPU instructions with partial architectural progress
/// (such as REP) issue separate committed accesses.
/// An MMIO backend must define supported access widths and side-effect ordering
/// before integration; this contract does not authorize retrying a device access.
pub trait GuestMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError>;

    /// The decoded instruction this backend already has for the instruction fetch at `rip` (the
    /// turbo engine of `od-machine` hands over the decode of an instruction it leaves to the
    /// general step), or `None` (the default: the decoder reads the bytes). A backend that answers
    /// must guarantee that the answer equals what [`crate::long_decode::decode`] would return now,
    /// byte for byte, including every right the fetch of those bytes needs. Wrappers do not
    /// forward this method.
    fn cached_instruction(
        &mut self,
        _rip: u64,
        _context: AccessContext,
    ) -> Option<crate::long_decode::Instruction> {
        None
    }

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

    /// Reads an integer destination only after validating its complete range for a guest write.
    /// Memory read-modify-write instructions use this to report write-protection faults before
    /// reading a read-only destination. Backends must validate all translated spans before
    /// returning data; unsupported backends refuse instead of falling back to a read-first path.
    fn read_integer_for_write(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: crate::OperandSize,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "write-intent integer read is unsupported".into(),
        ))
    }

    /// Checks whether a delayed descriptor Accessed-bit write can safely
    /// follow the supplied frame writes. Implementations must account for
    /// aliases and translation metadata, preserving earlier access failures.
    /// Backends without such a check refuse this delivery by default.
    fn preflight_descriptor_accessed_write(
        &mut self,
        _descriptor_access_address: u64,
        _descriptor_context: AccessContext,
        _stack_writes: &[(u64, usize)],
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "descriptor Accessed-bit write preflight is unsupported".into(),
        ))
    }

    /// Atomically exchanges one integer operand with guest memory and returns the old value.
    ///
    /// S6-20: a specialization of [`GuestMemory::atomic_rmw_integer`] (the provided method calls
    /// it), so a backend gets the exchange from the single RMW definition. A backend may still
    /// override it with a native operation; it must then return the previous value, perform one
    /// backend operation and leave memory unchanged on every error. The returned value is
    /// zero-extended for widths below qword. This contract is limited to serialization provided
    /// by the implementing backend; it does not by itself define SMP memory ordering.
    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_rmw_integer(address, context, size, &mut |_| replacement)
    }

    /// Atomically adds one integer operand to guest memory and returns the old value.
    ///
    /// S6-20: a specialization of [`GuestMemory::atomic_rmw_integer`] (the provided method calls
    /// it). An overriding implementation must perform one backend operation, wrap the stored
    /// result to `size`, and leave memory unchanged on every error. The returned value is
    /// zero-extended for widths below qword. This contract is limited to serialization provided
    /// by the implementing backend; it does not imply an x86 LOCK prefix or define SMP memory
    /// ordering. The S2-21f LOCK XADD form may use this operation only with private synchronous
    /// RAM and serial interpreter access, with no external observer/writer, DMA, shared alias,
    /// delayed write, or MMIO. This is an integration precondition, not a capability checked by
    /// this trait method.
    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_rmw_integer(address, context, size, &mut |old| {
            old.wrapping_add(addend) & size.mask()
        })
    }

    /// Atomically compares a guest integer with `expected` and conditionally replaces it.
    ///
    /// S6-20: a specialization of [`GuestMemory::atomic_rmw_integer`] (the provided method calls
    /// it). Implementations return the observed old value, compare against
    /// `expected & size.mask()`, and store `replacement & size.mask()` whenever they match (no
    /// spurious mismatch). A mismatch still requires write-access validation because this
    /// instruction form has a memory read-modify-write destination. The whole operation is one
    /// backend call; every error leaves guest RAM unchanged. This method does not define x86
    /// LOCK or SMP memory ordering.
    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let expected = expected & size.mask();
        self.atomic_rmw_integer(address, context, size, &mut |old| {
            if old == expected {
                replacement & size.mask()
            } else {
                old
            }
        })
    }

    /// S6-20: the single place that defines the atomicity of every `LOCK` read-modify-write
    /// instruction form and of `XCHG` with a memory operand (CPU_CONTRACT.md, "S6-20 -
    /// единый атомарный API LOCK"). It reads the integer at `address`, passes the old value to
    /// `update`, writes the returned value back (always, even when it equals the old value:
    /// SDM, a failed `CMPXCHG` still writes the destination, so a read-only page faults) and
    /// returns the old value. The write is validated before any visible effect: on every error
    /// memory is unchanged and `update` is not observable from outside.
    ///
    /// `update` is a pure function of the old value; an implementation that retries (a
    /// compare-and-replace loop) may call it more than once and only the value of the successful
    /// attempt is stored.
    ///
    /// The default implementation is the serial one for the interleaving mode (one host thread,
    /// the vCPU never switches inside an instruction): an ordinary read followed by an ordinary
    /// write. It is correct only because the scheduler runs whole instructions; a backend that
    /// shares RAM between host threads must override it with a host atomic operation, and no
    /// instruction code changes. S6-31: `od-machine`'s `PagedMovMemoryAdapter` does (a
    /// compare-exchange loop on the RAM cell; the three specializations are single host
    /// operations); this default stays for the private single-threaded backends.
    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: crate::OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        let old = self.read_integer(address, context, size)?;
        let new = update(old);
        self.write_integer(address, context, size, new)?;
        Ok(old)
    }

    /// S2-51: the 128-bit read-modify-write of `CMPXCHG16B` (CPU_CONTRACT.md, S2-51). It reads the
    /// 16 bytes at `address` (little endian: the low qword is at `address`), passes the old value
    /// to `update`, writes the returned value back (always, even when it equals the old value:
    /// SDM, the destination of CMPXCHG8B/CMPXCHG16B receives a write cycle without regard to the
    /// comparison, so a read-only page faults on a failed compare too) and returns the old value.
    ///
    /// The caller has already checked that `address` is 16-byte aligned (the SDM raises #GP(0)
    /// for a misaligned CMPXCHG16B operand but gives no order against the page checks; the order
    /// "alignment first, no memory access" is confirmed by the hardware oracle of S2-51, case
    /// unaligned_and_prot_none: #GP(0), not #PF), so the operand never crosses a page and the one
    /// 16-byte write is validated as a whole: on every error memory is unchanged.
    /// `update` is a pure function of the old value (an implementation that retries may call it
    /// more than once).
    ///
    /// The default implementation is the serial one (one ordinary read, one ordinary write): it
    /// gives single-processor atomicity because the vCPU never switches inside an instruction in
    /// the interleaving mode. It is NOT atomic across host threads: a backend that shares RAM
    /// between host threads (`od-machine`'s paged adapter in the parallel mode) must override it:
    /// that adapter refuses the operation in the parallel mode until a host 128-bit
    /// compare-exchange exists, which must come before CX16 is declared to a multiprocessor guest;
    /// SMP ordering of this form is stage 6, not S2-51.
    fn atomic_rmw_u128(
        &mut self,
        address: u64,
        context: AccessContext,
        update: &mut dyn FnMut(u128) -> u128,
    ) -> Result<u128, MemoryError> {
        let mut bytes = [0u8; 16];
        self.read(address, context, &mut bytes)?;
        let old = u128::from_le_bytes(bytes);
        let new = update(old);
        self.write(address, context, &new.to_le_bytes())?;
        Ok(old)
    }

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

    /// S6-32: the fence of the CPL0 profile set (`MFENCE`, `SFENCE`, `LFENCE`), one call per
    /// retired fence instruction, with the kind of the instruction. The interpreter knows nothing
    /// about execution modes: the memory decides. The default is a no-op, which is correct for a
    /// backend that runs all processors on one host thread (the interleaving mode: no host
    /// barrier is placed); a backend that shares RAM between host threads overrides it with the
    /// host fence its contract names (`od-machine`'s `PagedMovMemoryAdapter`: a host fence in the
    /// parallel mode, nothing in the interleaving mode). The call changes no guest RAM and no CPU
    /// state; an error is reported like any memory error, and the fence is then not retired. It
    /// is separate from [`GuestMemory::mfence`] (the S2-21g completion hook used outside the CPL0
    /// set), which keeps its meaning. Every wrapper around a `GuestMemory` must forward this call
    /// (as it forwards `FetchCapture`): a wrapper that forgot it silently swallows the fence,
    /// because the default is a no-op.
    fn fence(&mut self, _kind: FenceKind) -> Result<(), MemoryError> {
        Ok(())
    }

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

    /// Reads the destination of an unlocked read-modify-write using write permissions.
    /// A paging fault carries write intent. This step does not write the operand or set its
    /// page's dirty bit. Backends must opt in to this access.
    fn read_for_write_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: crate::OperandSize,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "write-intent integer read is unsupported".into(),
        ))
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
mod page_fault_tests {
    use super::*;

    fn context(privilege: PrivilegeLevel, origin: AccessOrigin) -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin,
        }
    }

    #[test]
    fn page_fault_error_codes_match_independent_literal_cases() {
        let supervisor = context(PrivilegeLevel::Ring0, AccessOrigin::Explicit);
        let user = context(PrivilegeLevel::Ring3, AccessOrigin::Explicit);
        assert_eq!(
            page_fault_error_code(AccessKind::Read, supervisor, false, false, false),
            0x00
        );
        assert_eq!(
            page_fault_error_code(AccessKind::Write, user, false, false, false),
            0x06
        );
        assert_eq!(
            page_fault_error_code(AccessKind::Write, user, true, false, false),
            0x07
        );
        assert_eq!(
            page_fault_error_code(AccessKind::Read, supervisor, true, true, false),
            0x09
        );
        assert_eq!(
            page_fault_error_code(AccessKind::Fetch, user, true, false, true),
            0x15
        );
        assert_eq!(
            page_fault_error_code(AccessKind::Fetch, user, false, false, false),
            0x04
        );
        assert_eq!(
            page_fault_error_code(AccessKind::Fetch, user, false, false, true),
            0x14
        );

        let descriptor = context(PrivilegeLevel::Ring3, AccessOrigin::DescriptorTable);
        assert_eq!(descriptor.privilege, PrivilegeLevel::Ring3);
        assert_eq!(descriptor.page_privilege(), PrivilegeLevel::Ring0);
        assert_eq!(
            page_fault_error_code(AccessKind::Read, descriptor, true, false, false),
            0x01
        );
    }

    #[test]
    fn page_fault_record_keeps_instruction_and_fault_addresses_distinct() {
        let context = context(PrivilegeLevel::Ring3, AccessOrigin::DescriptorTable);
        let record = PageFaultRecord::new(0x1234, AccessKind::Read, context, 0x01, 0xfeed_beef);
        assert_eq!(record.fault().vector, 14);
        assert_eq!(record.fault().error_code, Some(0x01));
        assert_eq!(record.saved_rip(), 0x1234);
        assert_eq!(record.cr2_value(), 0xfeed_beef);
        assert_eq!(record.context(), context);
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
