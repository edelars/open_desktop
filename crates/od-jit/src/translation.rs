//! Typed compile identity and caller-supplied code dependency snapshots.
//!
//! This module does not own guest mappings or implement a cache. Its caller
//! must provide stable identities and monotonically changing versions from the
//! code/mapping owner. Runtime CPU state is intentionally not part of a key.

use std::collections::HashMap;
use std::num::NonZeroU64;

use crate::block::{
    BasicBlock, BlockStopReason, BranchTargetResolution, CodeMemory, CodeView, GuestByteRange,
};
use od_isa::long_decode;
use od_isa::memory::AccessContext;

macro_rules! nonzero_identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(NonZeroU64);

        impl $name {
            pub const fn new(value: NonZeroU64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0.get()
            }
        }
    };
}

macro_rules! nonzero_version {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(NonZeroU64);

        impl $name {
            pub const fn new(value: NonZeroU64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0.get()
            }

            /// Advance without wrapping. The owner must replace or retire the
            /// identity if its generation space is exhausted.
            pub fn checked_next(self) -> Option<Self> {
                NonZeroU64::new(self.0.get().checked_add(1)?).map(Self)
            }
        }
    };
}

nonzero_identity!(AddressSpaceId);
nonzero_identity!(MappingId);
nonzero_identity!(BackingId);
nonzero_version!(MappingVersion);
nonzero_version!(BackingVersion);

/// Caller-owned nonzero generation for one frozen execution-context snapshot.
/// It is never a host pointer and must advance without wrapping across reset,
/// restore, or clone boundaries while a cache could retain compiled code.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExecutionContextId(NonZeroU64);

impl ExecutionContextId {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn from_u64(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub fn checked_next(self) -> Option<Self> {
        NonZeroU64::new(self.0.get().checked_add(1)?).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GuestExecutionMode {
    Long64,
    Compatibility,
    Protected,
    Real,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum JitSemanticProfile {
    S0_09Fixture,
    /// Opt-in composed NativeRegisterV1 fixture. Its translation keys are
    /// intentionally disjoint from the S0-09 fixture dispatcher.
    NativeRegisterV1Fixture,
    Other,
}

/// The bounded architectural context portion relevant to this fixture cache.
/// It intentionally does not model production control-register/MMU state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExecutionProfile {
    pub mode: GuestExecutionMode,
    pub canonical_address_bits: u8,
    pub jit_profile: JitSemanticProfile,
}

impl ExecutionProfile {
    pub const S0_09_LONG64_CANONICAL48: Self = Self {
        mode: GuestExecutionMode::Long64,
        canonical_address_bits: 48,
        jit_profile: JitSemanticProfile::S0_09Fixture,
    };

    pub const NATIVE_REGISTER_V1_LONG64_CANONICAL48: Self = Self {
        mode: GuestExecutionMode::Long64,
        canonical_address_bits: 48,
        jit_profile: JitSemanticProfile::NativeRegisterV1Fixture,
    };

    pub const fn is_supported_fixture(self) -> bool {
        matches!(self, Self::S0_09_LONG64_CANONICAL48)
    }

    pub const fn is_supported_cached_fixture(self) -> bool {
        matches!(
            self,
            Self::S0_09_LONG64_CANONICAL48 | Self::NATIVE_REGISTER_V1_LONG64_CANONICAL48
        )
    }
}

/// Caller-owned frozen stamp held stable for an entire synchronous dispatch.
/// The owner must keep its mapping/backing snapshot leased until dispatch
/// returns; code dependency identities and versions remain separately keyed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExecutionContextSnapshot {
    pub id: ExecutionContextId,
    pub profile: ExecutionProfile,
}

impl ExecutionContextSnapshot {
    pub const fn new(id: ExecutionContextId, profile: ExecutionProfile) -> Self {
        Self { id, profile }
    }

    pub const fn s0_09_fixture(id: ExecutionContextId) -> Self {
        Self::new(id, ExecutionProfile::S0_09_LONG64_CANONICAL48)
    }

    pub const fn native_register_v1_fixture(id: ExecutionContextId) -> Self {
        Self::new(id, ExecutionProfile::NATIVE_REGISTER_V1_LONG64_CANONICAL48)
    }
}

/// Compile modes whose admission/lowering assumptions can change output.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompileFlavor {
    S0_09NativeBlock,
    S0_09FixtureDispatch,
    /// Eager composed NativeRegisterV1 cached dispatch; separate from S0-09.
    S3_38bNativeRegisterV1Cached,
    /// Demand-flags NativeRegisterV1 cache candidate; never aliases eager code.
    S3_38cNativeRegisterV1DemandFlagsCachedCandidate,
    S0_09NopFallback,
    /// Uncached first V2 checked `MOV r64, [mem]` compiler profile.
    S3_19bMemoryLoadV2,
    /// Shared typed-IR/allocator V2 checked qword-load entry.
    S3_26aIrMemoryLoadV2,
    /// Shared typed-IR/allocator V2 exact `MOV EAX,[RBX]` dword-load entry.
    S3_26cIrDwordMemoryLoadV2,
    /// Shared typed-IR/allocator V2 exact `MOV [RBX],EAX` dword-store entry.
    S3_26eIrDwordMemoryStoreV2,
    /// Shared typed-IR/allocator V2 checked qword-store entry.
    S3_26bIrMemoryStoreV2,
    /// One of the two exact checked qword two-site IR entries.
    S3_26dIrTwoMemoryV2,
    /// Uncached first V2 checked `MOV [mem], r64` compiler profile.
    S3_20bMemoryStoreV2,
    /// Bounded active-code three-instruction fixture block with V3 exits.
    S3_24aActiveCodeV3,
}

/// Caller-owned identity/version metadata for one guest mapping span.
///
/// Aliases use distinct `mapping_id` values while sharing `backing_id` and
/// `backing_version`. IDs are scoped by the caller and must not be reused with
/// unchanged versions while a key can remain reachable.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CodeIdentity {
    pub address_space: AddressSpaceId,
    pub mapping: MappingId,
    pub mapping_version: MappingVersion,
    pub backing: BackingId,
    pub backing_version: BackingVersion,
    pub executable: bool,
}

/// Half-open guest range associated with one stable mapping/backing snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CodeDependencySpan {
    pub start_rip: u64,
    pub end_rip: u64,
    pub identity: CodeIdentity,
}

/// Terminal classification is explicit because it can affect emitted code
/// and deferred behavior even when entry RIP and the decoded prefix match.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalKind {
    Branch {
        target_rip: u64,
        resolution: BranchTargetResolution,
    },
    UnsupportedBranchTarget {
        target_rip: u64,
    },
    BufferEnd,
    InstructionLimit,
    ByteLimit,
    UnsupportedDecoded,
    UnsupportedEncoding,
    DecodeBoundary,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TerminalClassification {
    pub stop_rip: u64,
    pub kind: TerminalKind,
}

/// The actual instruction byte ranges consumed by one bounded build.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstructionFetchRanges(Vec<GuestByteRange>);

impl InstructionFetchRanges {
    pub fn as_slice(&self) -> &[GuestByteRange] {
        &self.0
    }
}

/// Immutable, fully comparable compile identity. It includes all source bytes
/// in the CodeView (including bytes after the current block), all dependency
/// spans covering that view, and the block's exact instruction fetch ranges.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranslationKey {
    flavor: CompileFlavor,
    execution_context: ExecutionContextSnapshot,
    entry_rip: u64,
    code_base: u64,
    code_end: u64,
    code_bytes: Vec<u8>,
    dependencies: Vec<CodeDependencySpan>,
    fetch_ranges: InstructionFetchRanges,
    terminal: TerminalClassification,
}

impl TranslationKey {
    pub fn for_block(
        code: CodeView<'_>,
        block: &BasicBlock,
        flavor: CompileFlavor,
        execution_context: ExecutionContextSnapshot,
        dependency_spans: &[CodeDependencySpan],
    ) -> Result<Self, TranslationKeyError> {
        if block.code_end != code.code_end() {
            return Err(TranslationKeyError::CodeEndMismatch {
                block: block.code_end,
                view: code.code_end(),
            });
        }
        if block.entry_rip < code.base_rip() || block.entry_rip > code.code_end() {
            return Err(TranslationKeyError::EntryOutsideCode {
                entry_rip: block.entry_rip,
                base_rip: code.base_rip(),
                code_end: code.code_end(),
            });
        }

        validate_decoded_source(code, block)?;
        let terminal = classify_terminal(block)?;
        let fetch_ranges = validate_fetch_ranges(code, block)?;
        let dependencies = validate_dependencies(code, dependency_spans)?;

        Ok(Self {
            flavor,
            execution_context,
            entry_rip: block.entry_rip,
            code_base: code.base_rip(),
            code_end: code.code_end(),
            code_bytes: code.bytes().to_vec(),
            dependencies,
            fetch_ranges,
            terminal,
        })
    }

    pub const fn flavor(&self) -> CompileFlavor {
        self.flavor
    }

    pub const fn execution_context(&self) -> ExecutionContextSnapshot {
        self.execution_context
    }

    pub const fn entry_rip(&self) -> u64 {
        self.entry_rip
    }

    pub const fn code_base(&self) -> u64 {
        self.code_base
    }

    pub const fn code_end(&self) -> u64 {
        self.code_end
    }

    pub fn code_bytes(&self) -> &[u8] {
        &self.code_bytes
    }

    pub fn dependencies(&self) -> &[CodeDependencySpan] {
        &self.dependencies
    }

    /// Validate a current owner snapshot against this key's complete
    /// dependency coverage. The caller separately compares the full key/source
    /// bytes; this method checks whether the mapped executable backing is
    /// still the same versioned object.
    pub fn dependencies_match_snapshot(
        &self,
        current: &[CodeDependencySpan],
    ) -> Result<bool, TranslationKeyError> {
        Ok(validate_dependency_spans(self.code_base, self.code_end, current)? == self.dependencies)
    }

    pub fn matches_code_view(&self, code: CodeView<'_>) -> bool {
        self.code_base == code.base_rip()
            && self.code_end == code.code_end()
            && self.code_bytes == code.bytes()
    }

    pub fn fetch_ranges(&self) -> &[GuestByteRange] {
        self.fetch_ranges.as_slice()
    }

    pub const fn terminal(&self) -> TerminalClassification {
        self.terminal
    }

    /// Logical bytes owned by this key, excluding allocator capacity and the
    /// caller-owned dependency snapshot passed during validation.
    pub fn logical_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(self.code_bytes.len())?
            .checked_add(
                self.dependencies
                    .len()
                    .checked_mul(std::mem::size_of::<CodeDependencySpan>())?,
            )?
            .checked_add(
                self.fetch_ranges
                    .as_slice()
                    .len()
                    .checked_mul(std::mem::size_of::<GuestByteRange>())?,
            )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranslationKeyError {
    CodeEndMismatch {
        block: u64,
        view: u64,
    },
    EntryOutsideCode {
        entry_rip: u64,
        base_rip: u64,
        code_end: u64,
    },
    EmptyBlock,
    InstructionRangeCountMismatch {
        instructions: usize,
        ranges: usize,
    },
    InvalidInstructionRange {
        index: usize,
        start_rip: u64,
        end_rip: u64,
    },
    NonContiguousInstructionRange {
        index: usize,
        expected_rip: u64,
        actual_rip: u64,
    },
    FetchRangeOutsideCode {
        start_rip: u64,
        end_rip: u64,
        base_rip: u64,
        code_end: u64,
    },
    SourceDecodeMismatch {
        index: usize,
        guest_rip: u64,
    },
    DecodedInstructionMismatch {
        index: usize,
        guest_rip: u64,
    },
    NoDependencySpans,
    EmptyDependencySpan {
        index: usize,
    },
    DependencyOutsideCode {
        index: usize,
        start_rip: u64,
        end_rip: u64,
        base_rip: u64,
        code_end: u64,
    },
    DependencyGap {
        expected_rip: u64,
        actual_rip: u64,
    },
    OverlappingDependencies {
        previous_end: u64,
        next_start: u64,
    },
    DependencyCoverageIncomplete {
        expected_end: u64,
        actual_end: u64,
    },
    NonExecutableDependency {
        index: usize,
    },
    InconsistentMappingSnapshot {
        address_space: AddressSpaceId,
        mapping: MappingId,
    },
    InconsistentBackingVersion {
        backing: BackingId,
    },
}

fn validate_fetch_ranges(
    code: CodeView<'_>,
    block: &BasicBlock,
) -> Result<InstructionFetchRanges, TranslationKeyError> {
    if block.instructions.is_empty() {
        return Err(TranslationKeyError::EmptyBlock);
    }
    if block.instructions.len() != block.guest_ranges.len() {
        return Err(TranslationKeyError::InstructionRangeCountMismatch {
            instructions: block.instructions.len(),
            ranges: block.guest_ranges.len(),
        });
    }

    let mut previous_end = block.entry_rip;
    for (index, (instruction, range)) in block
        .instructions
        .iter()
        .zip(block.guest_ranges.iter())
        .enumerate()
    {
        let range_length = range.end_rip.checked_sub(range.start_rip);
        if range.start_rip >= range.end_rip
            || instruction.guest_rip != range.start_rip
            || instruction.next_rip != range.end_rip
            || range_length != Some(u64::from(instruction.decoded.length))
        {
            return Err(TranslationKeyError::InvalidInstructionRange {
                index,
                start_rip: range.start_rip,
                end_rip: range.end_rip,
            });
        }
        if range.start_rip != previous_end {
            return Err(TranslationKeyError::NonContiguousInstructionRange {
                index,
                expected_rip: previous_end,
                actual_rip: range.start_rip,
            });
        }
        if range.start_rip < code.base_rip() || range.end_rip > code.code_end() {
            return Err(TranslationKeyError::FetchRangeOutsideCode {
                start_rip: range.start_rip,
                end_rip: range.end_rip,
                base_rip: code.base_rip(),
                code_end: code.code_end(),
            });
        }
        previous_end = range.end_rip;
    }
    Ok(InstructionFetchRanges(block.guest_ranges.clone()))
}

/// `BasicBlock` is public data and can be paired with the wrong CodeView.
/// Re-decode its records against this immutable source before using them as a
/// translation identity. Each decoder read is bounded to the claimed range,
/// so a mismatch cannot make validation consume a later instruction.
fn validate_decoded_source(
    code: CodeView<'_>,
    block: &BasicBlock,
) -> Result<(), TranslationKeyError> {
    for (index, record) in block.instructions.iter().enumerate() {
        let Some(mut memory) = CodeMemory::bounded_range(code, record.guest_rip, record.next_rip)
        else {
            return Err(TranslationKeyError::SourceDecodeMismatch {
                index,
                guest_rip: record.guest_rip,
            });
        };
        let decoded = long_decode::decode(&mut memory, record.guest_rip, AccessContext::default())
            .map_err(|_| TranslationKeyError::SourceDecodeMismatch {
                index,
                guest_rip: record.guest_rip,
            })?;
        let next_rip = record.guest_rip.checked_add(u64::from(decoded.length));
        if decoded != record.decoded || next_rip != Some(record.next_rip) {
            return Err(TranslationKeyError::DecodedInstructionMismatch {
                index,
                guest_rip: record.guest_rip,
            });
        }
    }
    Ok(())
}

fn classify_terminal(block: &BasicBlock) -> Result<TerminalClassification, TranslationKeyError> {
    let last_rip = block
        .instructions
        .last()
        .map_or(block.entry_rip, |instruction| instruction.next_rip);
    let branch_rip = block
        .instructions
        .last()
        .map_or(block.entry_rip, |instruction| instruction.guest_rip);
    let (stop_rip, kind) = match &block.stop_reason {
        BlockStopReason::Branch { target } => (
            branch_rip,
            TerminalKind::Branch {
                target_rip: target.rip,
                resolution: target.resolution,
            },
        ),
        BlockStopReason::UnsupportedBranchTarget {
            branch_rip,
            target_rip,
        } => (
            *branch_rip,
            TerminalKind::UnsupportedBranchTarget {
                target_rip: *target_rip,
            },
        ),
        BlockStopReason::BufferEnd => (last_rip, TerminalKind::BufferEnd),
        BlockStopReason::InstructionLimit => (last_rip, TerminalKind::InstructionLimit),
        BlockStopReason::ByteLimit => (last_rip, TerminalKind::ByteLimit),
        BlockStopReason::UnsupportedDecoded { guest_rip, .. } => {
            (*guest_rip, TerminalKind::UnsupportedDecoded)
        }
        BlockStopReason::UnsupportedEncoding { guest_rip, .. } => {
            (*guest_rip, TerminalKind::UnsupportedEncoding)
        }
        BlockStopReason::DecodeBoundary(boundary) => {
            (boundary.guest_rip, TerminalKind::DecodeBoundary)
        }
    };
    Ok(TerminalClassification { stop_rip, kind })
}

fn validate_dependencies(
    code: CodeView<'_>,
    spans: &[CodeDependencySpan],
) -> Result<Vec<CodeDependencySpan>, TranslationKeyError> {
    validate_dependency_spans(code.base_rip(), code.code_end(), spans)
}

fn validate_dependency_spans(
    code_base: u64,
    code_end: u64,
    spans: &[CodeDependencySpan],
) -> Result<Vec<CodeDependencySpan>, TranslationKeyError> {
    if spans.is_empty() {
        return Err(TranslationKeyError::NoDependencySpans);
    }
    let mut sorted = spans.to_vec();
    sorted.sort_by_key(|span| (span.start_rip, span.end_rip));

    let mut mapping_snapshots = HashMap::new();
    let mut backing_versions = HashMap::new();
    let mut expected_start = code_base;

    for (index, span) in sorted.iter().enumerate() {
        if span.start_rip >= span.end_rip {
            return Err(TranslationKeyError::EmptyDependencySpan { index });
        }
        if span.start_rip < code_base || span.end_rip > code_end || span.start_rip > code_end {
            return Err(TranslationKeyError::DependencyOutsideCode {
                index,
                start_rip: span.start_rip,
                end_rip: span.end_rip,
                base_rip: code_base,
                code_end,
            });
        }
        if !span.identity.executable {
            return Err(TranslationKeyError::NonExecutableDependency { index });
        }
        if span.start_rip < expected_start {
            return Err(TranslationKeyError::OverlappingDependencies {
                previous_end: expected_start,
                next_start: span.start_rip,
            });
        }
        if span.start_rip > expected_start {
            return Err(TranslationKeyError::DependencyGap {
                expected_rip: expected_start,
                actual_rip: span.start_rip,
            });
        }

        let mapping_key = (span.identity.address_space, span.identity.mapping);
        let mapping_snapshot = (
            span.identity.mapping_version,
            span.identity.backing,
            span.identity.backing_version,
            span.identity.executable,
        );
        if mapping_snapshots
            .insert(mapping_key, mapping_snapshot)
            .is_some_and(|previous| previous != mapping_snapshot)
        {
            return Err(TranslationKeyError::InconsistentMappingSnapshot {
                address_space: span.identity.address_space,
                mapping: span.identity.mapping,
            });
        }
        if backing_versions
            .insert(span.identity.backing, span.identity.backing_version)
            .is_some_and(|previous| previous != span.identity.backing_version)
        {
            return Err(TranslationKeyError::InconsistentBackingVersion {
                backing: span.identity.backing,
            });
        }
        expected_start = span.end_rip;
    }

    if expected_start != code_end {
        return Err(TranslationKeyError::DependencyCoverageIncomplete {
            expected_end: code_end,
            actual_end: expected_start,
        });
    }
    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockLimits, BuildOutcome, build_block};
    use od_isa::memory::AccessContext;
    use std::hash::{BuildHasherDefault, Hasher};

    fn nz(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("nonzero fixture value")
    }

    fn test_context() -> ExecutionContextSnapshot {
        ExecutionContextSnapshot::s0_09_fixture(
            ExecutionContextId::from_u64(1).expect("test context ID is nonzero"),
        )
    }

    fn identity(
        mapping: u64,
        mapping_version: u64,
        backing: u64,
        backing_version: u64,
        executable: bool,
    ) -> CodeIdentity {
        CodeIdentity {
            address_space: AddressSpaceId::new(nz(1)),
            mapping: MappingId::new(nz(mapping)),
            mapping_version: MappingVersion::new(nz(mapping_version)),
            backing: BackingId::new(nz(backing)),
            backing_version: BackingVersion::new(nz(backing_version)),
            executable,
        }
    }

    fn build(code: &[u8], entry: u64) -> (CodeView<'_>, BasicBlock) {
        build_with_limits(code, entry, BlockLimits::default())
    }

    fn build_with_limits(
        code: &[u8],
        entry: u64,
        limits: BlockLimits,
    ) -> (CodeView<'_>, BasicBlock) {
        let view = CodeView::new(0x4000, code).unwrap();
        let outcome = build_block(view, entry, AccessContext::default(), limits).unwrap();
        let BuildOutcome::Block(block) = outcome else {
            panic!("fixture must produce a block");
        };
        (view, block)
    }

    fn span(start: u64, end: u64, id: CodeIdentity) -> CodeDependencySpan {
        CodeDependencySpan {
            start_rip: start,
            end_rip: end,
            identity: id,
        }
    }

    fn key<'a>(
        code: CodeView<'a>,
        block: &BasicBlock,
        flavor: CompileFlavor,
        spans: &[CodeDependencySpan],
    ) -> Result<TranslationKey, TranslationKeyError> {
        key_in_context(code, block, flavor, test_context(), spans)
    }

    fn key_in_context<'a>(
        code: CodeView<'a>,
        block: &BasicBlock,
        flavor: CompileFlavor,
        context: ExecutionContextSnapshot,
        spans: &[CodeDependencySpan],
    ) -> Result<TranslationKey, TranslationKeyError> {
        TranslationKey::for_block(code, block, flavor, context, spans)
    }

    fn one_span(code: CodeView<'_>, id: CodeIdentity) -> [CodeDependencySpan; 1] {
        [span(code.base_rip(), code.code_end(), id)]
    }

    #[test]
    fn execution_context_id_is_nonzero_and_advances_without_wrapping() {
        assert!(ExecutionContextId::from_u64(0).is_none());
        let id = ExecutionContextId::from_u64(41).unwrap();
        assert_eq!(id.checked_next().unwrap().get(), 42);
        assert!(
            ExecutionContextId::from_u64(u64::MAX)
                .unwrap()
                .checked_next()
                .is_none()
        );
    }

    #[test]
    fn key_distinguishes_bytes_flavor_base_end_tail_and_terminal() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0, 0x90];
        let (code, block) = build(&bytes, 0x4000);
        let spans = one_span(code, identity(10, 1, 20, 1, true));
        let base = key(code, &block, CompileFlavor::S0_09NativeBlock, &spans).unwrap();
        assert_eq!(base.code_bytes(), bytes);
        assert_ne!(
            base,
            key(code, &block, CompileFlavor::S0_09FixtureDispatch, &spans).unwrap()
        );

        let changed_tail = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0, 0xf4];
        let (tail_view, tail_block) = build(&changed_tail, 0x4000);
        let tail_spans = one_span(tail_view, identity(10, 1, 20, 1, true));
        assert_ne!(
            base,
            key(
                tail_view,
                &tail_block,
                CompileFlavor::S0_09NativeBlock,
                &tail_spans
            )
            .unwrap()
        );

        let limit_bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0, 0xf4];
        let (limit_view, limit_block) = build_with_limits(
            &limit_bytes,
            0x4000,
            BlockLimits {
                instructions: 32,
                bytes: 10,
            },
        );
        let (default_view, default_block) = build(&limit_bytes, 0x4000);
        assert_eq!(limit_block.guest_ranges, default_block.guest_ranges);
        let limit_key = key(
            limit_view,
            &limit_block,
            CompileFlavor::S0_09NativeBlock,
            &one_span(limit_view, identity(10, 1, 20, 1, true)),
        )
        .unwrap();
        let default_key = key(
            default_view,
            &default_block,
            CompileFlavor::S0_09NativeBlock,
            &one_span(default_view, identity(10, 1, 20, 1, true)),
        )
        .unwrap();
        assert_ne!(limit_key.terminal(), default_key.terminal());
        assert_ne!(limit_key, default_key);

        let relocated = CodeView::new(0x5000, &bytes).unwrap();
        let BuildOutcome::Block(relocated_block) = build_block(
            relocated,
            0x5000,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap() else {
            panic!("fixture must produce a block");
        };
        let relocated_spans = one_span(relocated, identity(10, 1, 20, 1, true));
        assert_ne!(
            base,
            key(
                relocated,
                &relocated_block,
                CompileFlavor::S0_09NativeBlock,
                &relocated_spans
            )
            .unwrap()
        );

        let shorter = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (short_view, short_block) = build(&shorter, 0x4000);
        let short_spans = one_span(short_view, identity(10, 1, 20, 1, true));
        assert_ne!(
            base,
            key(
                short_view,
                &short_block,
                CompileFlavor::S0_09NativeBlock,
                &short_spans
            )
            .unwrap()
        );
    }

    #[test]
    fn translation_key_distinguishes_execution_context_and_profile() {
        let bytes = [0x48, 0xB8, 9, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let spans = one_span(code, identity(10, 1, 20, 1, true));
        let base = key(code, &block, CompileFlavor::S0_09FixtureDispatch, &spans).unwrap();
        let new_generation = key_in_context(
            code,
            &block,
            CompileFlavor::S0_09FixtureDispatch,
            ExecutionContextSnapshot::s0_09_fixture(ExecutionContextId::from_u64(2).unwrap()),
            &spans,
        )
        .unwrap();
        assert_ne!(base, new_generation);

        let new_profile = key_in_context(
            code,
            &block,
            CompileFlavor::S0_09FixtureDispatch,
            ExecutionContextSnapshot::new(
                test_context().id,
                ExecutionProfile {
                    mode: GuestExecutionMode::Long64,
                    canonical_address_bits: 57,
                    jit_profile: JitSemanticProfile::S0_09Fixture,
                },
            ),
            &spans,
        )
        .unwrap();
        assert_ne!(base, new_profile);
    }

    #[test]
    fn key_includes_mapping_backing_versions_and_permissions() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let base_spans = one_span(code, identity(10, 1, 20, 1, true));
        let base = key(code, &block, CompileFlavor::S0_09NativeBlock, &base_spans).unwrap();

        for altered in [
            identity(11, 1, 20, 1, true), // different mapping alias
            identity(10, 2, 20, 1, true), // remapped/reused address
            identity(10, 1, 21, 1, true), // different backing storage
            identity(10, 1, 20, 2, true), // changed backing generation
        ] {
            assert_ne!(
                base,
                key(
                    code,
                    &block,
                    CompileFlavor::S0_09NativeBlock,
                    &one_span(code, altered)
                )
                .unwrap()
            );
        }
        assert!(matches!(
            key(
                code,
                &block,
                CompileFlavor::S0_09NativeBlock,
                &one_span(code, identity(10, 1, 20, 1, false))
            ),
            Err(TranslationKeyError::NonExecutableDependency { .. })
        ));
    }

    #[test]
    fn aliases_share_backing_generation_but_keep_mapping_identity() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let first = identity(10, 1, 20, 8, true);
        let alias = identity(11, 1, 20, 8, true);
        let spans_a = [span(0x4000, 0x400a, first)];
        let key_a = key(code, &block, CompileFlavor::S0_09NativeBlock, &spans_a).unwrap();

        let alias_view = CodeView::new(0x5000, &bytes).unwrap();
        let BuildOutcome::Block(alias_block) = build_block(
            alias_view,
            0x5000,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap() else {
            panic!("alias fixture must produce a block");
        };
        let spans_b = [span(0x5000, 0x500a, alias)];
        let key_b = key(
            alias_view,
            &alias_block,
            CompileFlavor::S0_09NativeBlock,
            &spans_b,
        )
        .unwrap();
        assert_ne!(key_a, key_b);
        assert!(
            key_b
                .dependencies()
                .iter()
                .all(|dep| dep.identity.backing == BackingId::new(nz(20)))
        );
        assert!(
            key_b
                .dependencies()
                .iter()
                .all(|dep| dep.identity.backing_version == BackingVersion::new(nz(8)))
        );
    }

    #[test]
    fn dependency_ranges_must_cover_code_and_may_cross_multiple_spans() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let first = identity(10, 1, 20, 1, true);
        let second = identity(11, 1, 20, 1, true);
        let spans = [span(0x4000, 0x4005, first), span(0x4005, 0x400a, second)];
        let compiled = key(code, &block, CompileFlavor::S0_09NativeBlock, &spans).unwrap();
        assert_eq!(
            compiled.fetch_ranges(),
            &[GuestByteRange {
                start_rip: 0x4000,
                end_rip: 0x400a
            }]
        );

        assert!(matches!(
            key(
                code,
                &block,
                CompileFlavor::S0_09NativeBlock,
                &[span(0x4000, 0x4004, first), span(0x4005, 0x400a, second)]
            ),
            Err(TranslationKeyError::DependencyCoverageIncomplete { .. })
                | Err(TranslationKeyError::DependencyGap { .. })
        ));
        assert!(matches!(
            key(
                code,
                &block,
                CompileFlavor::S0_09NativeBlock,
                &[span(0x4000, 0x4006, first), span(0x4005, 0x400a, second)]
            ),
            Err(TranslationKeyError::OverlappingDependencies { .. })
        ));
    }

    #[test]
    fn rejects_overflow_inconsistent_versions_and_invalid_fetch_metadata() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let id_a = identity(10, 1, 20, 1, true);
        let id_b = identity(11, 1, 20, 2, true);
        let two_spans = [span(0x4000, 0x4005, id_a), span(0x4005, 0x400a, id_b)];
        assert!(matches!(
            key(code, &block, CompileFlavor::S0_09NativeBlock, &two_spans),
            Err(TranslationKeyError::InconsistentBackingVersion { .. })
        ));
        let inconsistent_mapping = [
            span(0x4000, 0x4005, identity(10, 1, 20, 1, true)),
            span(0x4005, 0x400a, identity(10, 2, 20, 1, true)),
        ];
        assert!(matches!(
            key(
                code,
                &block,
                CompileFlavor::S0_09NativeBlock,
                &inconsistent_mapping
            ),
            Err(TranslationKeyError::InconsistentMappingSnapshot { .. })
        ));
        assert!(matches!(
            key(
                code,
                &block,
                CompileFlavor::S0_09NativeBlock,
                &[span(0x4000, 0x400a, id_a), span(0x4000, 0x400a, id_a)]
            ),
            Err(TranslationKeyError::OverlappingDependencies { .. })
        ));
        assert_eq!(MappingVersion::new(nz(u64::MAX)).checked_next(), None);
        assert_eq!(
            BackingVersion::new(nz(1)).checked_next(),
            Some(BackingVersion::new(nz(2)))
        );
        assert!(CodeView::new(u64::MAX, &[0x90]).is_err());

        let mut malformed_block = block.clone();
        malformed_block.guest_ranges[0].end_rip += 1;
        assert!(matches!(
            key(
                code,
                &malformed_block,
                CompileFlavor::S0_09NativeBlock,
                &one_span(code, id_a)
            ),
            Err(TranslationKeyError::InvalidInstructionRange { .. })
        ));
    }

    #[test]
    fn rejects_decoder_records_from_another_same_length_code_view() {
        let code_a = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let code_b = [0x48, 0xb8, 2, 0, 0, 0, 0, 0, 0, 0];
        let (view_a, block_a) = build(&code_a, 0x4000);
        let (view_b, block_b) = build(&code_b, 0x4000);
        assert_eq!(view_a.len(), view_b.len());
        assert_eq!(block_a.guest_ranges, block_b.guest_ranges);
        let dependencies = one_span(view_b, identity(10, 1, 20, 1, true));

        assert!(matches!(
            key(
                view_b,
                &block_a,
                CompileFlavor::S0_09NativeBlock,
                &dependencies
            ),
            Err(TranslationKeyError::DecodedInstructionMismatch { .. })
        ));
        assert!(
            key(
                view_b,
                &block_b,
                CompileFlavor::S0_09NativeBlock,
                &dependencies
            )
            .is_ok()
        );
    }

    #[test]
    fn key_equality_compares_full_content_after_hash_collisions() {
        #[derive(Default)]
        struct ConstantHasher;
        impl Hasher for ConstantHasher {
            fn finish(&self) -> u64 {
                0
            }
            fn write(&mut self, _bytes: &[u8]) {}
        }

        let hasher = BuildHasherDefault::<ConstantHasher>::default();
        let first_bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let second_bytes = [0x48, 0xb8, 2, 0, 0, 0, 0, 0, 0, 0];
        let (first_code, first_block) = build(&first_bytes, 0x4000);
        let (second_code, second_block) = build(&second_bytes, 0x4000);
        let first = key(
            first_code,
            &first_block,
            CompileFlavor::S0_09NativeBlock,
            &one_span(first_code, identity(10, 1, 20, 1, true)),
        )
        .unwrap();
        let second = key(
            second_code,
            &second_block,
            CompileFlavor::S0_09NativeBlock,
            &one_span(second_code, identity(10, 1, 20, 1, true)),
        )
        .unwrap();
        let mut set = std::collections::HashSet::with_hasher(hasher);
        set.insert(first.clone());
        set.insert(second.clone());
        assert_ne!(first, second);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn runtime_state_is_not_an_input_to_translation_key() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let spans = one_span(code, identity(10, 1, 20, 1, true));
        let key_a = key(code, &block, CompileFlavor::S0_09NativeBlock, &spans).unwrap();
        let key_b = key(code, &block, CompileFlavor::S0_09NativeBlock, &spans).unwrap();
        // Runtime budget, STOP, GPR and RFLAGS are not accepted by this API;
        // equal compile inputs therefore remain equal across runtime calls.
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn dependency_snapshot_validation_reports_stale_without_rebuilding_key() {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let (code, block) = build(&bytes, 0x4000);
        let original = one_span(code, identity(10, 1, 20, 1, true));
        let key = key(code, &block, CompileFlavor::S0_09NativeBlock, &original).unwrap();
        assert!(key.dependencies_match_snapshot(&original).unwrap());

        let changed_mapping = one_span(code, identity(10, 2, 20, 1, true));
        assert!(!key.dependencies_match_snapshot(&changed_mapping).unwrap());
        let denied = one_span(code, identity(10, 1, 20, 1, false));
        assert!(matches!(
            key.dependencies_match_snapshot(&denied),
            Err(TranslationKeyError::NonExecutableDependency { .. })
        ));
    }
}
