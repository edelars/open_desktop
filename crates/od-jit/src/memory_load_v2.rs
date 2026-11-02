//! Verified compiler path for the first V2 checked-memory block profile.
//!
//! This deliberately accepts only one decoded `MOV r64, [base]` instruction
//! in the exact `48 8B /r` encoding profile. It does not extend V1 admission.

use od_isa::long_decode::{self, Operand, Operation};
use od_isa::memory::AccessContext;

use crate::arm64::{
    AddressBase as HostAddressBase, EncodeError, EncodedCode, Encoder, PairIndexMode, WReg, XReg,
    XRegOrSp, XRegOrZero,
};
use crate::block::{CodeMemory, CodeView};
use crate::runtime::{
    LONG_MODE_GPR_OFFSET, LONG_MODE_RIP_OFFSET, MEMORY_SITE_SOURCE_BYTES_MAX,
    MemorySiteOperationV2, MemorySiteV2, RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET,
    RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET, RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET,
    RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET, RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET,
    RUNTIME_CONTEXT_STOP_RIP_OFFSET, RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET,
    RUNTIME_CONTEXT_V3_CODE_DIRTY_OFFSET, RuntimeExitV3, memory_site_helper_v2,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryLoadCompileErrorV2 {
    EntryOutsideCode,
    Decode,
    UnsupportedForm,
    Encoding(EncodeError),
    PcMap(crate::statepoints::PcMapError),
    CodeRangeOverflow,
}

impl From<EncodeError> for MemoryLoadCompileErrorV2 {
    fn from(error: EncodeError) -> Self {
        Self::Encoding(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MemoryArtifactV2 {
    code: EncodedCode,
    site: MemorySiteV2,
    entry_rip: u64,
    source_bytes: [u8; MEMORY_SITE_SOURCE_BYTES_MAX],
    source_len: u8,
    compile_flavor: crate::translation::CompileFlavor,
}

/// Compiler-issued load artifact. Arbitrary code bytes cannot be promoted to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryLoadBlockV2 {
    artifact: MemoryArtifactV2,
    pc_map: crate::statepoints::PcMap,
}

/// Compiler-issued checked load lowered from the shared typed IR/allocator
/// path. Its entry and publication contract are distinct from direct V2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrMemoryLoadBlockV2 {
    artifact: MemoryArtifactV2,
    pc_map: crate::statepoints::PcMap,
}

/// Compiler-issued shared-IR artifact for exact unprefixed `8B 03`, `8B 00`,
/// and `8B 43 ib` dword loads. Its distinct type and compile flavor keep dword
/// admission separate from the qword load profiles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrMemoryDwordLoadBlockV2 {
    artifact: MemoryArtifactV2,
    pc_map: crate::statepoints::PcMap,
}

/// Compiler-issued shared-IR artifact for exact `89 03` (`MOV [RBX],EAX`)
/// and `89 00` (`MOV [RAX],EAX`).
/// Its distinct type and compile flavor keep dword stores separate from the
/// qword store profiles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrMemoryDwordStoreBlockV2 {
    artifact: MemoryArtifactV2,
    pc_map: crate::statepoints::PcMap,
}

/// Compiler-issued checked store lowered through shared typed IR and allocator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrMemoryStoreBlockV2 {
    artifact: MemoryArtifactV2,
    pc_map: crate::statepoints::PcMap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrMemoryTwoSiteBlockV2 {
    code: EncodedCode,
    entry_rip: u64,
    source_bytes: [u8; 6],
    sites: [MemorySiteV2; 2],
    sequence: crate::runtime::MemoryPairSequenceV2,
    pc_map: crate::statepoints::PcMap,
}

impl IrMemoryTwoSiteBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.code
    }
    pub const fn entry_rip(&self) -> u64 {
        self.entry_rip
    }
    pub fn source_bytes(&self) -> &[u8; 6] {
        &self.source_bytes
    }
    pub fn sites(&self) -> &[MemorySiteV2; 2] {
        &self.sites
    }
    pub const fn sequence(&self) -> crate::runtime::MemoryPairSequenceV2 {
        self.sequence
    }
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
    pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
        crate::translation::CompileFlavor::S3_26dIrTwoMemoryV2
    }
}

/// Compatibility type name retained for the original exact store→load entry.
pub type IrMemoryStoreThenLoadBlockV2 = IrMemoryTwoSiteBlockV2;

impl IrMemoryStoreBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.artifact.code
    }
    pub fn site(&self) -> &MemorySiteV2 {
        &self.artifact.site
    }
    pub const fn entry_rip(&self) -> u64 {
        self.artifact.entry_rip
    }
    pub fn source_bytes(&self) -> &[u8] {
        &self.artifact.source_bytes[..usize::from(self.artifact.source_len)]
    }
    pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
        self.artifact.compile_flavor
    }
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
}

impl IrMemoryLoadBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.artifact.code
    }
    pub fn site(&self) -> &MemorySiteV2 {
        &self.artifact.site
    }
    pub const fn entry_rip(&self) -> u64 {
        self.artifact.entry_rip
    }
    pub fn source_bytes(&self) -> &[u8] {
        &self.artifact.source_bytes[..usize::from(self.artifact.source_len)]
    }
    pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
        self.artifact.compile_flavor
    }
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
}

impl IrMemoryDwordLoadBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.artifact.code
    }
    pub fn site(&self) -> &MemorySiteV2 {
        &self.artifact.site
    }
    pub const fn entry_rip(&self) -> u64 {
        self.artifact.entry_rip
    }
    pub fn source_bytes(&self) -> &[u8] {
        &self.artifact.source_bytes[..usize::from(self.artifact.source_len)]
    }
    pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
        self.artifact.compile_flavor
    }
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
}

impl IrMemoryDwordStoreBlockV2 {
    pub fn code(&self) -> &EncodedCode {
        &self.artifact.code
    }
    pub fn site(&self) -> &MemorySiteV2 {
        &self.artifact.site
    }
    pub const fn entry_rip(&self) -> u64 {
        self.artifact.entry_rip
    }
    pub fn source_bytes(&self) -> &[u8] {
        &self.artifact.source_bytes[..usize::from(self.artifact.source_len)]
    }
    pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
        self.artifact.compile_flavor
    }
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
}

/// Compiler-issued store artifact, kept distinct from load publication/API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryStoreBlockV2 {
    artifact: MemoryArtifactV2,
    pc_map: crate::statepoints::PcMap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveCodeArtifactV3 {
    code: EncodedCode,
    pc_map: crate::statepoints::PcMap,
    entry_rip: u64,
    source_bytes: [u8; 23],
    site: MemorySiteV2,
    completed_prefix_value: u64,
    tail_value: u64,
    compile_flavor: crate::translation::CompileFlavor,
}

/// A verifier-issued active-code fixture artifact for exactly three decoded
/// instructions: `MOV RDX, imm64; MOV [RBX], RAX; MOV RCX, imm64`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCodeBlockV3 {
    artifact: ActiveCodeArtifactV3,
}

impl ActiveCodeBlockV3 {
    pub fn code(&self) -> &EncodedCode {
        &self.artifact.code
    }
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.artifact.pc_map
    }
    pub fn site(&self) -> &MemorySiteV2 {
        &self.artifact.site
    }
    pub const fn entry_rip(&self) -> u64 {
        self.artifact.entry_rip
    }
    pub fn source_bytes(&self) -> &[u8; 23] {
        &self.artifact.source_bytes
    }
    pub const fn completed_prefix_value(&self) -> u64 {
        self.artifact.completed_prefix_value
    }
    pub const fn tail_value(&self) -> u64 {
        self.artifact.tail_value
    }
    pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
        self.artifact.compile_flavor
    }
}

/// Compile the exact S3-24a 23-byte source block using three independent
/// decoder records. The returned type cannot be published as a V1 or V2 entry.
pub fn compile_active_code_block_v3(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<ActiveCodeBlockV3, MemoryLoadCompileErrorV2> {
    if entry_rip != code.base_rip() || code.len() != 23 {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let source: [u8; 23] = code
        .bytes()
        .try_into()
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    if source[..2] != [0x48, 0xBA]
        || source[10..13] != [0x48, 0x89, 0x03]
        || source[13..15] != [0x48, 0xB9]
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let prefix = decode_active_code_instruction(&source[..10], entry_rip)?;
    let store_rip = entry_rip
        .checked_add(10)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let store = decode_active_code_instruction(&source[10..13], store_rip)?;
    let tail_rip = entry_rip
        .checked_add(13)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let tail = decode_active_code_instruction(&source[13..], tail_rip)?;
    if prefix.length != 10 || store.length != 3 || tail.length != 10 {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    entry_rip
        .checked_add(23)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let prefix_value = match prefix.operation {
        Operation::MovImmediate {
            dst: Operand::Register(dst),
            value,
        } if dst.register() == od_isa::Register::Rdx
            && dst.size() == od_isa::OperandSize::Qword =>
        {
            value
        }
        _ => return Err(MemoryLoadCompileErrorV2::UnsupportedForm),
    };
    let (register, operand) = match store.operation {
        Operation::Mov {
            dst: Operand::Memory(operand),
            src: Operand::Register(src),
        } if src.register() == od_isa::Register::Rax
            && src.size() == od_isa::OperandSize::Qword =>
        {
            (src.register(), operand)
        }
        _ => return Err(MemoryLoadCompileErrorV2::UnsupportedForm),
    };
    let tail_value = match tail.operation {
        Operation::MovImmediate {
            dst: Operand::Register(dst),
            value,
        } if dst.register() == od_isa::Register::Rcx
            && dst.size() == od_isa::OperandSize::Qword =>
        {
            value
        }
        _ => return Err(MemoryLoadCompileErrorV2::UnsupportedForm),
    };
    let next_rip = store_rip
        .checked_add(u64::from(store.length))
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let site = MemorySiteV2 {
        // The nested checked-store helper has a one-site table; instruction
        // index is therefore local to this runtime site table.
        instruction_index: 0,
        guest_rip: store_rip,
        next_rip,
        operation: MemorySiteOperationV2::Store,
        destination: None,
        source: Some(register as u8),
        operand,
    };
    if !site.has_supported_profile()
        || site.operand().base != od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
        || site.operand().index.is_some()
        || site.operand().displacement != 0
        || !site.matches_source_bytes(&source[10..13], MemorySiteOperationV2::Store)
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let (code, pc_map) = emit_active_code_block_v3(&site, entry_rip, prefix_value, tail_value)?;
    Ok(ActiveCodeBlockV3 {
        artifact: ActiveCodeArtifactV3 {
            code,
            pc_map,
            entry_rip,
            source_bytes: source,
            site,
            completed_prefix_value: prefix_value,
            tail_value,
            compile_flavor: crate::translation::CompileFlavor::S3_24aActiveCodeV3,
        },
    })
}

fn decode_active_code_instruction(
    bytes: &[u8],
    rip: u64,
) -> Result<od_isa::long_decode::Instruction, MemoryLoadCompileErrorV2> {
    let view = CodeView::new(rip, bytes).map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let mut memory = CodeMemory::full_code(view);
    long_decode::decode(&mut memory, rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)
}

fn emit_active_code_block_v3(
    site: &MemorySiteV2,
    entry_rip: u64,
    prefix_value: u64,
    tail_value: u64,
) -> Result<(EncodedCode, crate::statepoints::PcMap), MemoryLoadCompileErrorV2> {
    use crate::statepoints::{
        GuestBoundaryPoint, GuestRipBoundary, GuestRipSource, HostCodeRange, PcMap,
        SnapshotAdmissibility, StatePhase, StatePointRange,
    };
    let mut encoder = Encoder::new();
    let stop_address = encoder.new_label()?;
    let stop_requested = encoder.new_label()?;
    let budget = encoder.new_label()?;
    let helper_failure = encoder.new_label()?;
    let code_invalidated = encoder.new_label()?;
    let stop_after_tail = encoder.new_label()?;
    let complete = encoder.new_label()?;

    // Keep the canonical context, state and stop pointer across the checked
    // helper. x19-x23 are callee-saved; the inner call preserves them too.
    encoder.stp_x(
        xr(19)?,
        xr(20)?,
        HostAddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.stp_x(
        xr(21)?,
        xr(22)?,
        HostAddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.stp_x(
        xr(23)?,
        xr(30)?,
        HostAddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.mov_x(xr(19)?, XRegOrZero::Reg(xr(0)?));
    // RuntimeContextV3 begins with RuntimeContextV2, whose first member is V1.
    encoder.ldr_x(XRegOrZero::Reg(xr(20)?), HostAddressBase::Reg(xr(19)?), 0)?;

    emit_v3_instruction_guard(
        &mut encoder,
        entry_rip,
        stop_address,
        stop_requested,
        budget,
    )?;
    let prefix_start = active_code_offset(&encoder)?;
    emit_imm64(&mut encoder, 9, prefix_value)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_GPR_OFFSET as u32 + 2 * 8,
    )?;
    emit_imm64(&mut encoder, 9, site.guest_rip())?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    decrement_remaining_and_retire(&mut encoder, 1)?;
    let prefix_commit_end = active_code_offset(&encoder)?;

    // The store is one checked access through the shared V2 helper-call
    // primitive. All V3 guards and architectural commit sequencing stay here.
    // Site RIP is the committed prefix boundary; stop-address / STOP / budget
    // are rechecked in the mandated order before accessing memory.
    emit_v3_instruction_guard(
        &mut encoder,
        site.guest_rip(),
        stop_address,
        stop_requested,
        budget,
    )?;
    let (helper_call_start, helper_call_end) =
        emit_checked_memory_site_call_v2(&mut encoder, site.instruction_index())?;
    let helper_call_start = active_word_offset_to_bytes(helper_call_start)?;
    let helper_call_end = active_word_offset_to_bytes(helper_call_end)?;
    encoder.cbnz_w(WReg::new(0)?, helper_failure)?;
    let store_commit_start = active_code_offset(&encoder)?;
    emit_imm64(&mut encoder, 9, site.next_rip())?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    decrement_remaining_and_retire(&mut encoder, 2)?;
    let store_commit_end = active_code_offset(&encoder)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_V3_CODE_DIRTY_OFFSET as u32,
    )?;
    encoder.cbnz_x(XRegOrZero::Reg(xr(9)?), code_invalidated)?;
    encoder.ldarb_w(WReg::new(9)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(9)?, stop_requested)?;

    emit_v3_instruction_guard(
        &mut encoder,
        site.next_rip(),
        stop_address,
        stop_requested,
        budget,
    )?;
    let tail_commit_start = active_code_offset(&encoder)?;
    emit_imm64(&mut encoder, 9, tail_value)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_GPR_OFFSET as u32 + 1 * 8,
    )?;
    emit_imm64(&mut encoder, 9, entry_rip + 23)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    decrement_remaining_and_retire(&mut encoder, 3)?;
    let tail_commit_end = active_code_offset(&encoder)?;
    encoder.ldarb_w(WReg::new(9)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(9)?, stop_after_tail)?;
    encoder.b(complete)?;

    let common_exit_start = active_code_offset(&encoder)?;
    encoder.bind(code_invalidated)?;
    let code_invalidated_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::CodeInvalidated as u16)?;
    encoder.bind(helper_failure)?;
    let helper_failure_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::MemoryOperation as u16)?;
    encoder.bind(stop_address)?;
    let stop_address_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::StopAddress as u16)?;
    encoder.bind(stop_requested)?;
    let stop_requested_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::StopRequested as u16)?;
    encoder.bind(budget)?;
    let budget_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::BudgetExhausted as u16)?;
    encoder.bind(stop_after_tail)?;
    let stop_after_tail_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::StopRequested as u16)?;
    encoder.bind(complete)?;
    let complete_start = active_code_offset(&encoder)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::Complete as u16)?;
    let code = encoder
        .finish()
        .map_err(MemoryLoadCompileErrorV2::Encoding)?;
    let code_len = code
        .words()
        .len()
        .checked_mul(4)
        .and_then(|len| u32::try_from(len).ok())
        .ok_or(MemoryLoadCompileErrorV2::CodeRangeOverflow)?;
    let entry_rip = entry_rip;
    let mut ranges = Vec::new();
    let mut push = |start, end, rip, instruction_index, phase, snapshot| {
        if start < end {
            ranges.push(StatePointRange {
                range: HostCodeRange::new(start, end),
                guest_rip: rip,
                instruction_index,
                phase,
                snapshot,
            });
        }
    };
    push(
        0,
        prefix_start,
        GuestRipSource::Fixed(entry_rip),
        Some(0),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        prefix_start,
        prefix_commit_end,
        GuestRipSource::Fixed(entry_rip),
        Some(0),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    push(
        prefix_commit_end,
        helper_call_start,
        GuestRipSource::Fixed(site.guest_rip()),
        Some(1),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );
    push(
        helper_call_start,
        helper_call_end,
        GuestRipSource::Fixed(site.guest_rip()),
        Some(1),
        StatePhase::HelperCall,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );
    push(
        helper_call_end,
        store_commit_start,
        GuestRipSource::Fixed(site.guest_rip()),
        Some(1),
        StatePhase::AfterHelper,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );
    push(
        store_commit_start,
        store_commit_end,
        GuestRipSource::Fixed(site.guest_rip()),
        Some(1),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    push(
        store_commit_end,
        tail_commit_start,
        GuestRipSource::Fixed(site.next_rip()),
        Some(2),
        StatePhase::BeforeInstruction,
        SnapshotAdmissibility::ReplayCurrentInstruction,
    );
    push(
        tail_commit_start,
        tail_commit_end,
        GuestRipSource::Fixed(site.next_rip()),
        Some(2),
        StatePhase::CommitInProgress,
        SnapshotAdmissibility::InadmissibleDuringCommit,
    );
    let completed_rip = entry_rip
        .checked_add(23)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    push(
        tail_commit_end,
        common_exit_start,
        GuestRipSource::Fixed(completed_rip),
        Some(2),
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        code_invalidated_start,
        helper_failure_start,
        GuestRipSource::Fixed(site.next_rip()),
        Some(1),
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        helper_failure_start,
        stop_address_start,
        GuestRipSource::Fixed(site.guest_rip()),
        Some(1),
        StatePhase::Exit,
        SnapshotAdmissibility::InadmissibleAfterEffect,
    );
    push(
        stop_address_start,
        stop_requested_start,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        stop_requested_start,
        budget_start,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        budget_start,
        stop_after_tail_start,
        GuestRipSource::CanonicalState,
        None,
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        stop_after_tail_start,
        complete_start,
        GuestRipSource::Fixed(completed_rip),
        Some(2),
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    push(
        complete_start,
        code_len,
        GuestRipSource::Fixed(completed_rip),
        Some(2),
        StatePhase::Exit,
        SnapshotAdmissibility::CanonicalBoundary,
    );
    drop(push);
    let boundaries = vec![
        GuestBoundaryPoint {
            host_offset: prefix_commit_end,
            rip: GuestRipBoundary::Fixed(site.guest_rip()),
            instruction_index: 0,
            committed_instructions: 1,
            phase: StatePhase::Committed,
            snapshot: SnapshotAdmissibility::CanonicalBoundary,
        },
        GuestBoundaryPoint {
            host_offset: store_commit_end,
            rip: GuestRipBoundary::Fixed(site.next_rip()),
            instruction_index: 1,
            committed_instructions: 2,
            phase: StatePhase::Committed,
            snapshot: SnapshotAdmissibility::CanonicalBoundary,
        },
        GuestBoundaryPoint {
            host_offset: tail_commit_end,
            rip: GuestRipBoundary::Fixed(completed_rip),
            instruction_index: 2,
            committed_instructions: 3,
            phase: StatePhase::Committed,
            snapshot: SnapshotAdmissibility::CanonicalBoundary,
        },
    ];
    let pc_map = PcMap::new(code_len as usize, 3, ranges, boundaries)
        .map_err(MemoryLoadCompileErrorV2::PcMap)?;
    Ok((code, pc_map))
}

fn active_code_offset(encoder: &Encoder) -> Result<u32, MemoryLoadCompileErrorV2> {
    active_word_offset_to_bytes(encoder.position())
}

fn active_word_offset_to_bytes(position: usize) -> Result<u32, MemoryLoadCompileErrorV2> {
    position
        .checked_mul(4)
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or(MemoryLoadCompileErrorV2::CodeRangeOverflow)
}

fn emit_v3_instruction_guard(
    encoder: &mut Encoder,
    rip: u64,
    stop_address: crate::arm64::Label,
    stop_requested: crate::arm64::Label,
    budget: crate::arm64::Label,
) -> Result<(), EncodeError> {
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET + RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET)
            as u32,
    )?;
    let no_stop_address = encoder.new_label()?;
    encoder.cbz_x(XRegOrZero::Reg(xr(9)?), no_stop_address)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(10)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET + RUNTIME_CONTEXT_STOP_RIP_OFFSET) as u32,
    )?;
    // Compare canonical RIP directly with the stop address.
    let _ = rip; // Host validation guarantees the expected RIP at each boundary.
    encoder.subs_x(XRegOrZero::Zero, xr(9)?, XRegOrZero::Reg(xr(10)?));
    encoder.b_cond(crate::arm64::Condition::Equal, stop_address)?;
    encoder.bind(no_stop_address)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(21)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET + RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET)
            as u32,
    )?;
    let no_stop_flag = encoder.new_label()?;
    encoder.cbz_x(XRegOrZero::Reg(xr(21)?), no_stop_flag)?;
    encoder.ldarb_w(WReg::new(9)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(9)?, stop_requested)?;
    encoder.bind(no_stop_flag)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET + RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET)
            as u32,
    )?;
    encoder.cbz_x(XRegOrZero::Reg(xr(9)?), budget)?;
    Ok(())
}

fn decrement_remaining_and_retire(encoder: &mut Encoder, retired: u16) -> Result<(), EncodeError> {
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET + RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET)
            as u32,
    )?;
    encoder.subs_imm_x(XRegOrZero::Reg(xr(9)?), XRegOrSp::Reg(xr(9)?), 1)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET + RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET)
            as u32,
    )?;
    encoder.movz_x(xr(9)?, retired, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        (crate::runtime::RUNTIME_CONTEXT_V2_BASE_OFFSET
            + RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET) as u32,
    )?;
    Ok(())
}

fn emit_return_v3(encoder: &mut Encoder, tag: u16) -> Result<(), EncodeError> {
    encoder.movz_x(xr(0)?, tag, 0)?;
    encoder.ldp_x(
        xr(23)?,
        xr(30)?,
        HostAddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ldp_x(
        xr(21)?,
        xr(22)?,
        HostAddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ldp_x(
        xr(19)?,
        xr(20)?,
        HostAddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ret(xr(30)?);
    Ok(())
}

macro_rules! impl_artifact_accessors {
    ($type:ty) => {
        impl $type {
            pub fn code(&self) -> &EncodedCode {
                &self.artifact.code
            }

            pub fn site(&self) -> &MemorySiteV2 {
                &self.artifact.site
            }

            pub const fn entry_rip(&self) -> u64 {
                self.artifact.entry_rip
            }

            pub fn source_bytes(&self) -> &[u8] {
                &self.artifact.source_bytes[..usize::from(self.artifact.source_len)]
            }

            pub const fn source_bytes_storage(&self) -> ([u8; MEMORY_SITE_SOURCE_BYTES_MAX], u8) {
                (self.artifact.source_bytes, self.artifact.source_len)
            }

            pub const fn compile_flavor(&self) -> crate::translation::CompileFlavor {
                self.artifact.compile_flavor
            }
        }
    };
}

impl_artifact_accessors!(MemoryLoadBlockV2);
impl_artifact_accessors!(MemoryStoreBlockV2);

impl MemoryLoadBlockV2 {
    /// Generated-state attribution for this code. A `ReplayCurrentInstruction`
    /// value identifies the guest state only; replay additionally requires the
    /// caller to establish synchronous ordinary RAM independently. This map
    /// does not classify `GuestMemory` implementations or authorize replay for
    /// MMIO or unknown memory classes.
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
}

impl MemoryStoreBlockV2 {
    /// Generated-state attribution for this code. The map does not classify
    /// memory types or change the existing private synchronous store contract.
    pub fn pc_map(&self) -> &crate::statepoints::PcMap {
        &self.pc_map
    }
}

/// Compile the exact checked-memory V2 profile from decoded guest bytes.
///
/// The decoder remains the source of operand semantics. The raw-byte checks
/// narrow admission to the explicitly reviewed form; they do not decode it.
pub fn compile_memory_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<MemoryLoadBlockV2, MemoryLoadCompileErrorV2> {
    let (artifact, pc_map) =
        compile_memory_operation_v2(code, entry_rip, MemorySiteOperationV2::Load)?;
    Ok(MemoryLoadBlockV2 { artifact, pc_map })
}

/// Compile the exact checked qword load through BasicBlock → typed IR →
/// allocator → runtime emitter. This is an independent V2 artifact type.
pub fn compile_ir_memory_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryLoadBlockV2, MemoryLoadCompileErrorV2> {
    let (site, bytes) = checked_load_metadata_v2(code, entry_rip)?;
    let emitted = crate::block_compile::compile_checked_load_block_v2(code, entry_rip)
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let mut source_bytes = [0; MEMORY_SITE_SOURCE_BYTES_MAX];
    source_bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(IrMemoryLoadBlockV2 {
        artifact: MemoryArtifactV2 {
            code: emitted.code().clone(),
            site,
            entry_rip,
            source_bytes,
            source_len: bytes.len() as u8,
            compile_flavor: crate::translation::CompileFlavor::S3_26aIrMemoryLoadV2,
        },
        pc_map: emitted.pc_map().clone(),
    })
}

/// Compile exact unprefixed `8B 03` (`MOV EAX,[RBX]`), `8B 00`
/// (`MOV EAX,[RAX]`), `8B 43 ib` (`MOV EAX,[RBX+disp8]`), `8B 83 id`
/// (`MOV EAX,[RBX+disp32]`), `8B 05 id` (`MOV EAX,[RIP+disp32]`), or
/// `67 8B 03` (`MOV EAX,[EBX]`) or `67 8B 43 ib`
/// (`MOV EAX,[EBX+disp8]`) or `67 8B 83 id`
/// (`MOV EAX,[EBX+disp32]`)
/// through the
/// common block builder, typed IR, allocator, and runtime helper emitter.
pub fn compile_ir_memory_dword_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryDwordLoadBlockV2, MemoryLoadCompileErrorV2> {
    let (site, bytes) = checked_dword_load_metadata_v2(code, entry_rip)?;
    let emitted = crate::block_compile::compile_checked_dword_load_block_v2(code, entry_rip)
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let mut source_bytes = [0; MEMORY_SITE_SOURCE_BYTES_MAX];
    source_bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(IrMemoryDwordLoadBlockV2 {
        artifact: MemoryArtifactV2 {
            code: emitted.code().clone(),
            site,
            entry_rip,
            source_bytes,
            source_len: bytes.len() as u8,
            compile_flavor: crate::translation::CompileFlavor::S3_26cIrDwordMemoryLoadV2,
        },
        pc_map: emitted.pc_map().clone(),
    })
}

fn checked_dword_load_metadata_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<(MemorySiteV2, &[u8]), MemoryLoadCompileErrorV2> {
    let offset = entry_rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < code.len())
        .ok_or(MemoryLoadCompileErrorV2::EntryOutsideCode)?;
    let bytes = code
        .bytes()
        .get(offset..)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    if bytes != [0x8b, 0x03]
        && bytes != [0x8b, 0x00]
        && bytes != [0x67, 0x8b, 0x03]
        && !(bytes.len() == 4 && bytes[..3] == [0x67, 0x8b, 0x43])
        && !(bytes.len() == 7 && bytes[..3] == [0x67, 0x8b, 0x83])
        && !(bytes.len() == 3 && bytes[..2] == [0x8b, 0x43])
        && !(bytes.len() == 6 && bytes[..2] == [0x8b, 0x83])
        && !(bytes.len() == 6 && bytes[..2] == [0x8b, 0x05])
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let mut memory = CodeMemory::full_code(code);
    let decoded = long_decode::decode(&mut memory, entry_rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)?;
    if !crate::block::is_checked_dword_load_form(decoded) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let next_rip = entry_rip
        .checked_add(decoded.length.into())
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let Operation::Mov {
        dst: Operand::Register(destination),
        src: Operand::Memory(operand),
    } = decoded.operation
    else {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    };
    let site = MemorySiteV2 {
        instruction_index: 0,
        guest_rip: entry_rip,
        next_rip,
        operation: MemorySiteOperationV2::Load,
        destination: Some(destination.register() as u8),
        source: None,
        operand,
    };
    if !site.matches_source_bytes(bytes, MemorySiteOperationV2::Load) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    Ok((site, bytes))
}

/// Compile exact `89 03`, `89 00`, `89 43 ib`, `89 83 id`, `89 05 id`
/// (`MOV [RIP+disp32],EAX`), `67 89 03` (`MOV [EBX],EAX`), or `67 89 43 ib`
/// (`MOV [EBX+disp8],EAX`) through the common block builder, typed IR,
/// allocator, and runtime helper emitter.
pub fn compile_ir_memory_dword_store_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryDwordStoreBlockV2, MemoryLoadCompileErrorV2> {
    let offset = entry_rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < code.len())
        .ok_or(MemoryLoadCompileErrorV2::EntryOutsideCode)?;
    let bytes = code
        .bytes()
        .get(offset..)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    if bytes != [0x89, 0x03]
        && bytes != [0x89, 0x00]
        && bytes != [0x67, 0x89, 0x03]
        && !(bytes.len() == 4 && bytes[..3] == [0x67, 0x89, 0x43])
        && !(bytes.len() == 3 && bytes[..2] == [0x89, 0x43])
        && !(bytes.len() == 6 && bytes[..2] == [0x89, 0x83])
        && !(bytes.len() == 6 && bytes[..2] == [0x89, 0x05])
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let mut memory = CodeMemory::full_code(code);
    let decoded = long_decode::decode(&mut memory, entry_rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)?;
    if !crate::block::is_checked_dword_store_form(decoded) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let next_rip = entry_rip
        .checked_add(u64::from(decoded.length))
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let Operation::Mov {
        dst: Operand::Memory(operand),
        src: Operand::Register(source),
    } = decoded.operation
    else {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    };
    let site = MemorySiteV2 {
        instruction_index: 0,
        guest_rip: entry_rip,
        next_rip,
        operation: MemorySiteOperationV2::Store,
        destination: None,
        source: Some(source.register() as u8),
        operand,
    };
    if !site.matches_source_bytes(bytes, MemorySiteOperationV2::Store) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let emitted = crate::block_compile::compile_checked_dword_store_block_v2(code, entry_rip)
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let mut source_bytes = [0; MEMORY_SITE_SOURCE_BYTES_MAX];
    source_bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(IrMemoryDwordStoreBlockV2 {
        artifact: MemoryArtifactV2 {
            code: emitted.code().clone(),
            site,
            entry_rip,
            source_bytes,
            source_len: bytes.len() as u8,
            compile_flavor: crate::translation::CompileFlavor::S3_26eIrDwordMemoryStoreV2,
        },
        pc_map: emitted.pc_map().clone(),
    })
}

/// Compile exactly one checked qword store through BasicBlock → typed IR →
/// allocator → runtime emitter. The artifact has its own entry/API type.
pub fn compile_ir_memory_store_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryStoreBlockV2, MemoryLoadCompileErrorV2> {
    let (site, bytes) = checked_memory_metadata_v2(code, entry_rip, MemorySiteOperationV2::Store)?;
    if !(crate::block::exact_rip_relative_qword_rsi_rdi_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_direct_qword_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_direct_qword_store_rsi_rdi_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
    ) || crate::block::exact_qword_address32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rbp_base_no_index_sib_disp_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rsi_rdi_base_no_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_sib_displacement_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_absolute_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_absolute_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_indexed_no_base_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_indexed_no_base_rbp_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_indexed_no_base_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_indexed_no_base_rbp_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rsp_base_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rsp_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rbp_base_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rbp_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rsi_rdi_base_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_low_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_low_base_no_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rsi_rdi_base_no_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rbp_base_no_index_sib_disp_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_rsi_rdi_base_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_low_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_low_base_no_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rsi_rdi_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rsp_base_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rsp_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rbp_base_rbp_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    ) || crate::block::exact_qword_address32_rbp_base_rsi_rdi_index_sib_memory_bytes(
        code,
        entry_rip,
        bytes.len() as u8,
        0x89,
    )) || !site.has_supported_profile()
        || !site.matches_source_bytes(bytes, MemorySiteOperationV2::Store)
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let emitted = crate::block_compile::compile_checked_store_block_v2(code, entry_rip)
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let mut source_bytes = [0; MEMORY_SITE_SOURCE_BYTES_MAX];
    source_bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(IrMemoryStoreBlockV2 {
        artifact: MemoryArtifactV2 {
            code: emitted.code().clone(),
            site,
            entry_rip,
            source_bytes,
            source_len: bytes.len() as u8,
            compile_flavor: crate::translation::CompileFlavor::S3_26bIrMemoryStoreV2,
        },
        pc_map: emitted.pc_map().clone(),
    })
}

/// Compile exactly `48 89 03 48 8B 0B` as one checked store-then-load entry.
pub fn compile_ir_memory_store_then_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryStoreThenLoadBlockV2, MemoryLoadCompileErrorV2> {
    if code.bytes() != [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b] {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    compile_ir_memory_two_site_block_v2(code, entry_rip)
}

/// Compile one exact reviewed qword-memory pair.
pub fn compile_ir_memory_two_site_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryTwoSiteBlockV2, MemoryLoadCompileErrorV2> {
    if entry_rip != code.base_rip()
        || code.len() != 6
        || !matches!(
            code.bytes(),
            [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b]
                | [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02]
                | [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b]
        )
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let second_rip = entry_rip
        .checked_add(3)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let first_view = CodeView::new(entry_rip, &code.bytes()[..3])
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let second_view = CodeView::new(second_rip, &code.bytes()[3..])
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let store_then_load = code.bytes() == [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b];
    let load_then_store = code.bytes() == [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02];
    let (first_operation, second_operation) = if store_then_load {
        (MemorySiteOperationV2::Store, MemorySiteOperationV2::Load)
    } else {
        (
            MemorySiteOperationV2::Load,
            if load_then_store {
                MemorySiteOperationV2::Store
            } else {
                MemorySiteOperationV2::Load
            },
        )
    };
    let (mut first, _) = checked_memory_metadata_v2(first_view, entry_rip, first_operation)?;
    let (mut second, _) = checked_memory_metadata_v2(second_view, second_rip, second_operation)?;
    let rbx = od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx);
    let operands_match = if store_then_load {
        first.source() == Some(0)
            && first.operand().base == rbx
            && second.destination() == Some(1)
            && second.operand().base == rbx
    } else if load_then_store {
        first.destination() == Some(0)
            && first.operand().base == rbx
            && second.source() == Some(0)
            && second.operand().base
                == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rdx)
    } else {
        first.destination() == Some(0)
            && first.operand().base == rbx
            && second.destination() == Some(1)
            && second.operand().base == rbx
    };
    if !operands_match {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    first.instruction_index = 0;
    second.instruction_index = 1;
    let emitted = crate::block_compile::compile_checked_two_memory_block_v2(code, entry_rip)
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    Ok(IrMemoryTwoSiteBlockV2 {
        code: emitted.code().clone(),
        entry_rip,
        source_bytes: code
            .bytes()
            .try_into()
            .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?,
        sites: [first, second],
        sequence: if store_then_load {
            crate::runtime::MemoryPairSequenceV2::StoreThenLoad
        } else if load_then_store {
            crate::runtime::MemoryPairSequenceV2::LoadThenStore
        } else {
            crate::runtime::MemoryPairSequenceV2::LoadThenLoad
        },
        pc_map: emitted.pc_map().clone(),
    })
}

fn checked_load_metadata_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<(MemorySiteV2, &[u8]), MemoryLoadCompileErrorV2> {
    let offset = entry_rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < code.len())
        .ok_or(MemoryLoadCompileErrorV2::EntryOutsideCode)?;
    let bytes = code
        .bytes()
        .get(offset..)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    if !(3..=MEMORY_SITE_SOURCE_BYTES_MAX).contains(&bytes.len()) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let mut memory = CodeMemory::full_code(code);
    let decoded = long_decode::decode(&mut memory, entry_rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)?;
    if decoded.length as usize != bytes.len()
        || !crate::block::is_checked_qword_load_form(decoded)
        || !(crate::block::exact_rip_relative_qword_rsi_rdi_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_direct_qword_load_rsi_rdi_bytes(
            code,
            entry_rip,
            decoded.length,
        ) || crate::block::exact_direct_qword_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_sib_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_rbp_base_no_index_sib_disp_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_rsi_rdi_base_no_index_sib_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_sib_displacement_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_absolute_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_absolute_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_indexed_no_base_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_indexed_no_base_rbp_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_indexed_no_base_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        ) || crate::block::exact_qword_address32_indexed_no_base_rbp_sib_disp32_memory_bytes(
            code,
            entry_rip,
            decoded.length,
            0x8b,
        )
            || crate::block::exact_qword_address32_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rsp_base_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rsp_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rbp_base_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rbp_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rsi_rdi_base_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_low_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_low_base_no_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rsi_rdi_base_no_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rbp_base_no_index_sib_disp_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_rsi_rdi_base_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_low_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_low_base_no_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rsi_rdi_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rsp_base_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rsp_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rbp_base_rbp_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            )
            || crate::block::exact_qword_address32_rbp_base_rsi_rdi_index_sib_memory_bytes(
                code,
                entry_rip,
                decoded.length,
                0x8b,
            ))
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let (destination, operand) = match decoded.operation {
        Operation::Mov {
            dst: Operand::Register(destination),
            src: Operand::Memory(operand),
        } => (destination, operand),
        _ => return Err(MemoryLoadCompileErrorV2::UnsupportedForm),
    };
    let next_rip = entry_rip
        .checked_add(u64::from(decoded.length))
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let site = MemorySiteV2 {
        instruction_index: 0,
        guest_rip: entry_rip,
        next_rip,
        operation: MemorySiteOperationV2::Load,
        destination: Some(destination.register() as u8),
        source: None,
        operand,
    };
    if !site.matches_source_bytes(bytes, MemorySiteOperationV2::Load) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    Ok((site, bytes))
}

/// Compile the exact checked-memory V2 qword store profile.
pub fn compile_memory_store_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<MemoryStoreBlockV2, MemoryLoadCompileErrorV2> {
    let (artifact, pc_map) =
        compile_memory_operation_v2(code, entry_rip, MemorySiteOperationV2::Store)?;
    Ok(MemoryStoreBlockV2 { artifact, pc_map })
}

fn compile_memory_operation_v2(
    code: CodeView<'_>,
    entry_rip: u64,
    access: MemorySiteOperationV2,
) -> Result<(MemoryArtifactV2, crate::statepoints::PcMap), MemoryLoadCompileErrorV2> {
    let (site, bytes) = checked_memory_metadata_v2(code, entry_rip, access)?;
    let (code, pc_map) = emit_memory_operation_v2(&site)?;
    let compile_flavor = match access {
        MemorySiteOperationV2::Load => crate::translation::CompileFlavor::S3_19bMemoryLoadV2,
        MemorySiteOperationV2::Store => crate::translation::CompileFlavor::S3_20bMemoryStoreV2,
    };
    let mut source_bytes = [0; MEMORY_SITE_SOURCE_BYTES_MAX];
    source_bytes[..bytes.len()].copy_from_slice(bytes);
    Ok((
        MemoryArtifactV2 {
            code,
            site,
            entry_rip,
            source_bytes,
            source_len: bytes.len() as u8,
            compile_flavor,
        },
        pc_map,
    ))
}

/// Decode and validate one checked MOV memory site without emitting native
/// code. Both standalone V2 and shared-IR compilers use this metadata source.
fn checked_memory_metadata_v2(
    code: CodeView<'_>,
    entry_rip: u64,
    access: MemorySiteOperationV2,
) -> Result<(MemorySiteV2, &[u8]), MemoryLoadCompileErrorV2> {
    let offset = entry_rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < code.len())
        .ok_or(MemoryLoadCompileErrorV2::EntryOutsideCode)?;
    let instruction_bytes_len = code.len() - offset;
    if !(3..=MEMORY_SITE_SOURCE_BYTES_MAX).contains(&instruction_bytes_len) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let mut memory = CodeMemory::full_code(code);
    let decoded = long_decode::decode(&mut memory, entry_rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)?;
    let bytes = code
        .bytes()
        .get(offset..offset.saturating_add(instruction_bytes_len))
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    if decoded.length as usize != instruction_bytes_len {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let (register, operand) = match (access, decoded.operation) {
        (
            MemorySiteOperationV2::Load,
            Operation::Mov {
                dst: Operand::Register(destination),
                src: Operand::Memory(operand),
            },
        ) if destination.size() == od_isa::OperandSize::Qword => (destination.register(), operand),
        (
            MemorySiteOperationV2::Store,
            Operation::Mov {
                dst: Operand::Memory(operand),
                src: Operand::Register(source),
            },
        ) if source.size() == od_isa::OperandSize::Qword => (source.register(), operand),
        _ => return Err(MemoryLoadCompileErrorV2::UnsupportedForm),
    };
    let next_rip = entry_rip
        .checked_add(u64::from(decoded.length))
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let site = MemorySiteV2 {
        instruction_index: 0,
        guest_rip: entry_rip,
        next_rip,
        operation: access,
        destination: (access == MemorySiteOperationV2::Load).then_some(register as u8),
        source: (access == MemorySiteOperationV2::Store).then_some(register as u8),
        operand,
    };
    if !site.matches_source_bytes(bytes, access) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    Ok((site, bytes))
}

fn emit_memory_operation_v2(
    site: &MemorySiteV2,
) -> Result<(EncodedCode, crate::statepoints::PcMap), MemoryLoadCompileErrorV2> {
    use crate::statepoints::{
        GuestBoundaryPoint, GuestRipBoundary, GuestRipSource, HostCodeRange, PcMap,
        SnapshotAdmissibility, StatePhase, StatePointRange,
    };
    let mut encoder = Encoder::new();
    let budget_exit = encoder.new_label()?;
    let stop_exit = encoder.new_label()?;
    let stop_address_exit = encoder.new_label()?;
    let stop_after_commit = encoder.new_label()?;
    let helper_failure = encoder.new_label()?;
    let success = encoder.new_label()?;
    let no_stop_flag = encoder.new_label()?;

    encoder.stp_x(
        xr(19)?,
        xr(20)?,
        HostAddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.stp_x(
        xr(21)?,
        xr(30)?,
        HostAddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.mov_x(xr(19)?, XRegOrZero::Reg(xr(0)?));
    encoder.ldr_x(XRegOrZero::Reg(xr(20)?), HostAddressBase::Reg(xr(19)?), 0)?;

    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET as u32,
    )?;
    let no_stop_address = encoder.new_label()?;
    encoder.cbz_x(XRegOrZero::Reg(xr(9)?), no_stop_address)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(10)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_STOP_RIP_OFFSET as u32,
    )?;
    encoder.subs_x(XRegOrZero::Zero, xr(9)?, XRegOrZero::Reg(xr(10)?));
    encoder.b_cond(crate::arm64::Condition::Equal, stop_address_exit)?;
    encoder.bind(no_stop_address)?;

    encoder.ldr_x(
        XRegOrZero::Reg(xr(21)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET as u32,
    )?;
    encoder.cbz_x(XRegOrZero::Reg(xr(21)?), no_stop_flag)?;
    encoder.ldarb_w(WReg::new(9)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(9)?, stop_exit)?;
    encoder.bind(no_stop_flag)?;

    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.cbz_x(XRegOrZero::Reg(xr(9)?), budget_exit)?;
    encoder.bind(success)?;

    let helper_start = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    emit_checked_memory_site_call_v2(&mut encoder, site.instruction_index)?;
    encoder.cbnz_w(WReg::new(0)?, helper_failure)?;
    let helper_end = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;

    let commit_start = helper_end;

    if site.operation == MemorySiteOperationV2::Load {
        encoder.ldr_x(
            XRegOrZero::Reg(xr(9)?),
            HostAddressBase::Reg(xr(19)?),
            RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET as u32,
        )?;
        encoder.str_x(
            XRegOrZero::Reg(xr(9)?),
            HostAddressBase::Reg(xr(20)?),
            LONG_MODE_GPR_OFFSET as u32
                + u32::from(
                    site.destination
                        .ok_or(EncodeError::InvalidRegister(u8::MAX))?,
                ) * 8,
        )?;
    }
    emit_imm64(&mut encoder, 9, site.next_rip)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.subs_imm_x(XRegOrZero::Reg(xr(9)?), XRegOrSp::Reg(xr(9)?), 1)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(xr(9)?, 1, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    let committed_offset = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    encoder.ldarb_w(WReg::new(10)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(10)?, stop_after_commit)?;
    emit_return(&mut encoder, 0)?;

    encoder.bind(budget_exit)?;
    let budget_exit_start = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(xr(9)?, 0, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    emit_return(&mut encoder, 1)?;

    encoder.bind(stop_exit)?;
    let stop_exit_start = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(xr(9)?, 0, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    emit_return(&mut encoder, 3)?;

    encoder.bind(stop_after_commit)?;
    let stop_after_commit_start = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    emit_return(&mut encoder, 3)?;

    encoder.bind(stop_address_exit)?;
    let stop_address_exit_start = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(xr(9)?, 0, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    emit_return(&mut encoder, 4)?;

    encoder.bind(helper_failure)?;
    let helper_failure_start = encoder
        .position()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(xr(9)?, 0, 0)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(19)?),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    emit_return(
        &mut encoder,
        crate::runtime::RUNTIME_EXIT_V2_MEMORY_OPERATION as u16,
    )?;
    let code = encoder.finish()?;
    let code_len = code
        .words()
        .len()
        .checked_mul(4)
        .ok_or(EncodeError::BufferTooLarge)?;
    let mut ranges = Vec::new();
    let mut push_range =
        |start: usize, end: usize, phase, rip, snapshot| -> Result<(), MemoryLoadCompileErrorV2> {
            if start < end {
                let start = u32::try_from(start).map_err(|_| EncodeError::BufferTooLarge)?;
                let end = u32::try_from(end).map_err(|_| EncodeError::BufferTooLarge)?;
                ranges.push(StatePointRange {
                    range: HostCodeRange::new(start, end),
                    guest_rip: rip,
                    instruction_index: Some(site.instruction_index),
                    phase,
                    snapshot,
                });
            }
            Ok(())
        };
    let load = site.operation == MemorySiteOperationV2::Load;
    let boundary = u32::try_from(committed_offset).map_err(|_| EncodeError::BufferTooLarge)?;
    push_range(
        0,
        helper_start,
        StatePhase::BeforeInstruction,
        GuestRipSource::Fixed(site.guest_rip),
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    push_range(
        helper_start,
        helper_end,
        StatePhase::HelperCall,
        GuestRipSource::Fixed(site.guest_rip),
        if load {
            SnapshotAdmissibility::ReplayCurrentInstruction
        } else {
            SnapshotAdmissibility::InadmissibleAfterEffect
        },
    )?;
    push_range(
        commit_start,
        boundary as usize,
        StatePhase::CommitInProgress,
        GuestRipSource::Fixed(site.guest_rip),
        SnapshotAdmissibility::InadmissibleDuringCommit,
    )?;
    let code_len = u32::try_from(code_len).map_err(|_| EncodeError::BufferTooLarge)?;
    push_range(
        boundary as usize,
        budget_exit_start,
        StatePhase::Committed,
        GuestRipSource::Fixed(site.next_rip),
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    push_range(
        budget_exit_start,
        stop_exit_start,
        StatePhase::Exit,
        GuestRipSource::Fixed(site.guest_rip),
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    push_range(
        stop_exit_start,
        stop_after_commit_start,
        StatePhase::Exit,
        GuestRipSource::Fixed(site.guest_rip),
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    push_range(
        stop_after_commit_start,
        stop_address_exit_start,
        StatePhase::Exit,
        GuestRipSource::Fixed(site.next_rip),
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    push_range(
        stop_address_exit_start,
        helper_failure_start,
        StatePhase::Exit,
        GuestRipSource::Fixed(site.guest_rip),
        SnapshotAdmissibility::CanonicalBoundary,
    )?;
    push_range(
        helper_failure_start,
        code_len as usize,
        StatePhase::Exit,
        GuestRipSource::Fixed(site.guest_rip),
        if load {
            SnapshotAdmissibility::ReplayCurrentInstruction
        } else {
            SnapshotAdmissibility::InadmissibleAfterEffect
        },
    )?;
    let pc_map = PcMap::new(
        code_len as usize,
        1,
        ranges,
        vec![GuestBoundaryPoint {
            host_offset: boundary,
            rip: GuestRipBoundary::Fixed(site.next_rip),
            instruction_index: site.instruction_index,
            committed_instructions: 1,
            phase: StatePhase::Committed,
            snapshot: SnapshotAdmissibility::CanonicalBoundary,
        }],
    )
    .map_err(MemoryLoadCompileErrorV2::PcMap)?;
    Ok((code, pc_map))
}

/// Shared low-level call primitive. It only transports the V2 context/site to
/// the checked helper and returns its raw helper status in w0; callers retain
/// ownership of guards, architectural commits, retirement and exits.
fn emit_checked_memory_site_call_v2(
    encoder: &mut Encoder,
    site_index: u32,
) -> Result<(usize, usize), EncodeError> {
    encoder.mov_x(xr(0)?, XRegOrZero::Reg(xr(19)?));
    encoder.movz_x(xr(1)?, site_index as u16, 0)?;
    let helper = memory_site_helper_v2 as *const () as usize as u64;
    encoder.movz_x(xr(16)?, helper as u16, 0)?;
    encoder.movk_x(xr(16)?, (helper >> 16) as u16, 1)?;
    encoder.movk_x(xr(16)?, (helper >> 32) as u16, 2)?;
    encoder.movk_x(xr(16)?, (helper >> 48) as u16, 3)?;
    let call_start = encoder.position();
    encoder.blr(xr(16)?);
    Ok((call_start, encoder.position()))
}

fn emit_return(encoder: &mut Encoder, tag: u16) -> Result<(), EncodeError> {
    encoder.movz_x(xr(0)?, tag, 0)?;
    encoder.ldp_x(
        xr(21)?,
        xr(30)?,
        HostAddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ldp_x(
        xr(19)?,
        xr(20)?,
        HostAddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ret(xr(30)?);
    Ok(())
}

fn emit_imm64(encoder: &mut Encoder, register: u8, value: u64) -> Result<(), EncodeError> {
    let reg = xr(register)?;
    encoder.movz_x(reg, value as u16, 0)?;
    encoder.movk_x(reg, (value >> 16) as u16, 1)?;
    encoder.movk_x(reg, (value >> 32) as u16, 2)?;
    encoder.movk_x(reg, (value >> 48) as u16, 3)?;
    Ok(())
}

fn xr(number: u8) -> Result<XReg, EncodeError> {
    XReg::new(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statepoints::{GuestRipBoundary, GuestRipSource, SnapshotAdmissibility, StatePhase};
    use od_isa::Register;
    use od_isa::long_decode::AddressBase;

    fn matches_two_site_pc_map_contract(
        map: &crate::statepoints::PcMap,
        entry_rip: u64,
        operations: [MemorySiteOperationV2; 2],
    ) -> bool {
        use crate::statepoints::{
            GuestRipBoundary, GuestRipSource, SnapshotAdmissibility, StatePhase,
        };

        let code_len = map.code_len();
        if map.instruction_count() != 2
            || code_len == 0
            || code_len % 4 != 0
            || map.ranges().is_empty()
        {
            return false;
        }

        let mut cursor = 0;
        for (index, range) in map.ranges().iter().enumerate() {
            if range.range.start() != cursor
                || range.range.start() >= range.range.end()
                || range.range.start() % 4 != 0
                || range.range.end() % 4 != 0
                || range.range.end() > code_len
                || map.lookup(range.range.start()) != Some(range)
            {
                return false;
            }
            if index + 1 < map.ranges().len()
                && map.lookup(range.range.end()) != map.ranges().get(index + 1)
            {
                return false;
            }
            cursor = range.range.end();
        }
        if cursor != code_len
            || map.lookup(code_len).is_some()
            || map.snapshot_admissibility_at(code_len).is_some()
        {
            return false;
        }

        for index in 0..2u32 {
            let guest_rip = entry_rip + u64::from(index) * 3;
            for (phase, snapshot, expected_rip) in [
                (
                    StatePhase::BeforeInstruction,
                    SnapshotAdmissibility::CanonicalBoundary,
                    guest_rip,
                ),
                (
                    StatePhase::PendingEffects,
                    SnapshotAdmissibility::ReplayCurrentInstruction,
                    guest_rip,
                ),
                (
                    StatePhase::HelperCall,
                    SnapshotAdmissibility::InadmissibleAfterEffect,
                    guest_rip,
                ),
                (
                    StatePhase::AfterHelper,
                    SnapshotAdmissibility::InadmissibleAfterEffect,
                    guest_rip,
                ),
                (
                    StatePhase::CommitInProgress,
                    SnapshotAdmissibility::InadmissibleDuringCommit,
                    guest_rip,
                ),
                (
                    StatePhase::Committed,
                    SnapshotAdmissibility::CanonicalBoundary,
                    guest_rip + 3,
                ),
            ] {
                let matching: Vec<_> = map
                    .ranges()
                    .iter()
                    .filter(|range| range.instruction_index == Some(index) && range.phase == phase)
                    .collect();
                if matching.len() != 1
                    || matching[0].guest_rip != GuestRipSource::Fixed(expected_rip)
                    || matching[0].snapshot != snapshot
                {
                    return false;
                }
            }

            let failed_helper: Vec<_> = map
                .ranges()
                .iter()
                .filter(|range| {
                    range.instruction_index == Some(index)
                        && range.phase == StatePhase::Exit
                        && range.guest_rip == GuestRipSource::Fixed(guest_rip)
                        && range.snapshot == SnapshotAdmissibility::ReplayCurrentInstruction
                })
                .collect();
            if failed_helper.len() != 1 {
                return false;
            }
        }

        let shared_service: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Service)
            .collect();
        if shared_service.len() != 2
            || shared_service.iter().any(|range| {
                range.guest_rip != GuestRipSource::CanonicalState
                    || range.instruction_index.is_some()
                    || range.snapshot != SnapshotAdmissibility::CanonicalBoundary
            })
        {
            return false;
        }

        let shared_entry_exit: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Exit && range.instruction_index.is_none())
            .collect();
        if shared_entry_exit.len() != 1
            || shared_entry_exit[0].guest_rip != GuestRipSource::Fixed(entry_rip)
            || shared_entry_exit[0].snapshot != SnapshotAdmissibility::CanonicalBoundary
        {
            return false;
        }

        let boundaries = map.boundaries();
        if boundaries.len() != 2
            || boundaries[0].rip != GuestRipBoundary::Fixed(entry_rip + 3)
            || boundaries[0].instruction_index != 0
            || boundaries[0].committed_instructions != 1
            || boundaries[0].phase != StatePhase::Committed
            || boundaries[0].snapshot != SnapshotAdmissibility::CanonicalBoundary
            || boundaries[1].rip != GuestRipBoundary::Fixed(entry_rip + 6)
            || boundaries[1].instruction_index != 1
            || boundaries[1].committed_instructions != 2
            || boundaries[1].phase != StatePhase::Committed
            || boundaries[1].snapshot != SnapshotAdmissibility::CanonicalBoundary
        {
            return false;
        }
        for boundary in boundaries {
            let Some(range) = map.lookup(boundary.host_offset) else {
                return false;
            };
            if range.phase != StatePhase::Committed
                || range.instruction_index != Some(boundary.instruction_index)
                || map.snapshot_admissibility_at(boundary.host_offset)
                    != Some(SnapshotAdmissibility::CanonicalBoundary)
            {
                return false;
            }
        }

        let Some(store_index) = operations
            .iter()
            .position(|operation| *operation == MemorySiteOperationV2::Store)
        else {
            return true;
        };
        let store_index = store_index as u32;
        !map.ranges().iter().any(|range| {
            range.instruction_index == Some(store_index)
                && range.phase == StatePhase::HelperCall
                && range.snapshot == SnapshotAdmissibility::ReplayCurrentInstruction
        })
    }

    fn matches_single_site_pc_map_contract(
        map: &crate::statepoints::PcMap,
        entry_rip: u64,
        operation: MemorySiteOperationV2,
    ) -> bool {
        use crate::statepoints::{
            GuestRipBoundary, GuestRipSource, SnapshotAdmissibility, StatePhase,
        };

        let expected_helper_snapshot = match operation {
            MemorySiteOperationV2::Load => SnapshotAdmissibility::ReplayCurrentInstruction,
            MemorySiteOperationV2::Store => SnapshotAdmissibility::InadmissibleAfterEffect,
        };
        let expected_after_helper_snapshot = expected_helper_snapshot;
        let next_rip = entry_rip + 3;
        let code_len = map.code_len();
        if map.instruction_count() != 1
            || code_len == 0
            || code_len % 4 != 0
            || map.ranges().is_empty()
        {
            return false;
        }

        let mut cursor = 0;
        for (index, range) in map.ranges().iter().enumerate() {
            let attribution_is_valid = if range.phase == StatePhase::Service {
                range.instruction_index.is_none()
                    && (range.guest_rip == GuestRipSource::Fixed(entry_rip)
                        || range.guest_rip == GuestRipSource::CanonicalState)
            } else {
                range.instruction_index == Some(0)
            };
            if range.range.start() != cursor
                || range.range.start() >= range.range.end()
                || range.range.start() % 4 != 0
                || range.range.end() % 4 != 0
                || range.range.end() > code_len
                || map.lookup(range.range.start()) != Some(range)
                || !attribution_is_valid
            {
                return false;
            }
            if index + 1 < map.ranges().len()
                && map.lookup(range.range.end()) != map.ranges().get(index + 1)
            {
                return false;
            }
            cursor = range.range.end();
        }
        if cursor != code_len
            || map.lookup(code_len).is_some()
            || map.snapshot_admissibility_at(code_len).is_some()
        {
            return false;
        }

        let exactly_one = |phase, guest_rip, snapshot| {
            let matching = map
                .ranges()
                .iter()
                .filter(|range| range.instruction_index == Some(0) && range.phase == phase)
                .collect::<Vec<_>>();
            matching.len() == 1
                && matching[0].guest_rip == GuestRipSource::Fixed(guest_rip)
                && matching[0].snapshot == snapshot
        };
        if !exactly_one(
            StatePhase::BeforeInstruction,
            entry_rip,
            SnapshotAdmissibility::CanonicalBoundary,
        ) || !exactly_one(StatePhase::HelperCall, entry_rip, expected_helper_snapshot)
            || !exactly_one(
                StatePhase::AfterHelper,
                entry_rip,
                expected_after_helper_snapshot,
            )
            || !exactly_one(
                StatePhase::CommitInProgress,
                entry_rip,
                SnapshotAdmissibility::InadmissibleDuringCommit,
            )
        {
            return false;
        }

        let pending = map
            .ranges()
            .iter()
            .filter(|range| {
                range.instruction_index == Some(0) && range.phase == StatePhase::PendingEffects
            })
            .collect::<Vec<_>>();
        if pending.is_empty()
            || pending.iter().any(|range| {
                range.guest_rip != GuestRipSource::Fixed(entry_rip)
                    || range.snapshot != SnapshotAdmissibility::ReplayCurrentInstruction
            })
        {
            return false;
        }

        let services = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Service)
            .collect::<Vec<_>>();
        if services.len() != 2
            || services[0].guest_rip != GuestRipSource::Fixed(entry_rip)
            || services[0].instruction_index.is_some()
            || services[0].snapshot != SnapshotAdmissibility::CanonicalBoundary
            || services[1].guest_rip != GuestRipSource::CanonicalState
            || services[1].instruction_index.is_some()
            || services[1].snapshot != SnapshotAdmissibility::CanonicalBoundary
        {
            return false;
        }

        let exits = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Exit)
            .collect::<Vec<_>>();
        let failed_helper_exit = exits.iter().any(|range| {
            range.instruction_index == Some(0)
                && range.guest_rip == GuestRipSource::Fixed(entry_rip)
                // The failure edge is taken only when GuestMemory::write
                // returned an error; its contract guarantees no RAM change.
                && range.snapshot == SnapshotAdmissibility::ReplayCurrentInstruction
        });
        let committed_exit = exits.iter().any(|range| {
            range.guest_rip == GuestRipSource::Fixed(next_rip)
                && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
        });
        let exits_are_attributed = exits.iter().all(|range| {
            range.instruction_index == Some(0)
                && match range.guest_rip {
                    GuestRipSource::Fixed(rip) if rip == entry_rip => {
                        range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                            || range.snapshot == SnapshotAdmissibility::ReplayCurrentInstruction
                    }
                    GuestRipSource::Fixed(rip) if rip == next_rip => {
                        range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                    }
                    GuestRipSource::CanonicalState => {
                        operation == MemorySiteOperationV2::Load
                            && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                    }
                    _ => false,
                }
        });
        let canonical_block_complete_exit_count = exits
            .iter()
            .filter(|range| {
                range.guest_rip == GuestRipSource::CanonicalState
                    && range.instruction_index == Some(0)
                    && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
            })
            .count();
        let expected_canonical_block_complete_exit_count =
            usize::from(operation == MemorySiteOperationV2::Load);
        if !failed_helper_exit
            || !committed_exit
            || !exits_are_attributed
            || canonical_block_complete_exit_count != expected_canonical_block_complete_exit_count
        {
            return false;
        }

        let boundaries = map.boundaries();
        boundaries.len() == 1
            && boundaries[0].rip == GuestRipBoundary::Fixed(next_rip)
            && boundaries[0].instruction_index == 0
            && boundaries[0].committed_instructions == 1
            && boundaries[0].phase == StatePhase::Committed
            && boundaries[0].snapshot == SnapshotAdmissibility::CanonicalBoundary
            && map.lookup(boundaries[0].host_offset).is_some_and(|range| {
                matches!(range.phase, StatePhase::Committed | StatePhase::Exit)
                    && range.guest_rip == GuestRipSource::Fixed(next_rip)
                    && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
            })
    }

    fn matches_dword_pc_map_contract(
        map: &crate::statepoints::PcMap,
        entry_rip: u64,
        next_rip: u64,
        instruction_index: u32,
        instruction_count: u32,
        helper_phase: StatePhase,
        operation: MemorySiteOperationV2,
    ) -> bool {
        use crate::statepoints::{
            GuestRipBoundary, GuestRipSource, SnapshotAdmissibility, StatePhase,
        };

        let helper_snapshot = match operation {
            MemorySiteOperationV2::Load => SnapshotAdmissibility::ReplayCurrentInstruction,
            MemorySiteOperationV2::Store => SnapshotAdmissibility::InadmissibleAfterEffect,
        };
        let code_len = map.code_len();
        if map.instruction_count() != instruction_count || code_len == 0 || code_len % 4 != 0 {
            return false;
        }
        if helper_phase != StatePhase::HelperCall {
            return false;
        }

        let mut cursor = 0;
        for (range_index, range) in map.ranges().iter().enumerate() {
            let expected_site = if range.phase == StatePhase::Service {
                range.instruction_index.is_none()
            } else {
                range.instruction_index == Some(instruction_index)
            };
            if !expected_site
                || range.range.start() != cursor
                || range.range.start() >= range.range.end()
                || range.range.start() % 4 != 0
                || range.range.end() % 4 != 0
                || range.range.end() > code_len
                || map.lookup(range.range.start()) != Some(range)
            {
                return false;
            }
            if range_index + 1 < map.ranges().len()
                && map.lookup(range.range.end()) != map.ranges().get(range_index + 1)
            {
                return false;
            }
            cursor = range.range.end();
        }
        if cursor != code_len
            || map.lookup(code_len).is_some()
            || map.snapshot_admissibility_at(code_len).is_some()
        {
            return false;
        }

        let has_exactly_one = |phase, rip, snapshot| {
            let matching: Vec<_> = map
                .ranges()
                .iter()
                .filter(|range| {
                    range.instruction_index == Some(instruction_index) && range.phase == phase
                })
                .collect();
            matching.len() == 1
                && matching[0].guest_rip == GuestRipSource::Fixed(rip)
                && matching[0].snapshot == snapshot
        };
        let service_ranges: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Service)
            .collect();
        if service_ranges.len() != 2
            || service_ranges[0].guest_rip != GuestRipSource::Fixed(entry_rip)
            || service_ranges[0].snapshot != SnapshotAdmissibility::CanonicalBoundary
            || service_ranges[1].guest_rip != GuestRipSource::CanonicalState
            || service_ranges[1].snapshot != SnapshotAdmissibility::CanonicalBoundary
            || !has_exactly_one(
                StatePhase::BeforeInstruction,
                entry_rip,
                SnapshotAdmissibility::CanonicalBoundary,
            )
            || !has_exactly_one(helper_phase, entry_rip, helper_snapshot)
            || !has_exactly_one(StatePhase::AfterHelper, entry_rip, helper_snapshot)
            || !has_exactly_one(
                StatePhase::CommitInProgress,
                entry_rip,
                SnapshotAdmissibility::InadmissibleDuringCommit,
            )
        {
            return false;
        }

        let pending: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::PendingEffects)
            .collect();
        if pending.is_empty()
            || pending.iter().any(|range| {
                range.guest_rip != GuestRipSource::Fixed(entry_rip)
                    || range.snapshot != SnapshotAdmissibility::ReplayCurrentInstruction
            })
        {
            return false;
        }

        let failed_helper_snapshot = match operation {
            MemorySiteOperationV2::Load | MemorySiteOperationV2::Store => {
                SnapshotAdmissibility::ReplayCurrentInstruction
            }
        };
        let helper_failure_exits = map
            .ranges()
            .iter()
            .filter(|range| {
                range.phase == StatePhase::Exit
                    && range.guest_rip == GuestRipSource::Fixed(entry_rip)
                    && range.snapshot == failed_helper_snapshot
            })
            .count();
        let next_rip_exits: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| {
                range.phase == StatePhase::Exit
                    && range.guest_rip == GuestRipSource::Fixed(next_rip)
            })
            .collect();
        let exits: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Exit)
            .collect();
        let exits_have_exact_attribution = exits.iter().all(|range| {
            range.instruction_index == Some(instruction_index)
                && match range.guest_rip {
                    GuestRipSource::Fixed(rip) if rip == entry_rip => {
                        range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                            || range.snapshot == SnapshotAdmissibility::ReplayCurrentInstruction
                    }
                    GuestRipSource::Fixed(rip) if rip == next_rip => {
                        range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                    }
                    GuestRipSource::CanonicalState => {
                        operation == MemorySiteOperationV2::Load
                            && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
                    }
                    _ => false,
                }
        });
        let canonical_block_complete_exit_count = exits
            .iter()
            .filter(|range| {
                range.guest_rip == GuestRipSource::CanonicalState
                    && range.instruction_index == Some(instruction_index)
                    && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
            })
            .count();
        let expected_next_rip_exit_count = match operation {
            MemorySiteOperationV2::Load => 1,
            MemorySiteOperationV2::Store => 2,
        };
        let expected_canonical_block_complete_exit_count =
            usize::from(operation == MemorySiteOperationV2::Load);
        let boundary = map.boundaries();
        helper_failure_exits == 1
            && exits_have_exact_attribution
            && !map.ranges().iter().any(|range| {
                range.phase == StatePhase::Exit
                    && range.guest_rip == GuestRipSource::Fixed(entry_rip)
                    && range.snapshot == SnapshotAdmissibility::InadmissibleAfterEffect
            })
            && next_rip_exits.len() == expected_next_rip_exit_count
            && next_rip_exits.iter().all(|range| {
                range.instruction_index == Some(instruction_index)
                    && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
            })
            && canonical_block_complete_exit_count == expected_canonical_block_complete_exit_count
            && boundary.len() == 1
            && boundary[0].rip == GuestRipBoundary::Fixed(next_rip)
            && boundary[0].instruction_index == instruction_index
            && boundary[0].committed_instructions == 1
            && boundary[0].phase == StatePhase::Committed
            && boundary[0].snapshot == SnapshotAdmissibility::CanonicalBoundary
            && map.lookup(boundary[0].host_offset).is_some_and(|range| {
                range.instruction_index == Some(instruction_index)
                    && range.guest_rip == GuestRipSource::Fixed(next_rip)
                    && range.snapshot == SnapshotAdmissibility::CanonicalBoundary
            })
    }

    #[test]
    fn ir_dword_load_admits_only_exact_8b03_and_preserves_its_provenance() {
        let entry = 0x8100;
        let code = CodeView::new(entry, &[0x8b, 0x03]).unwrap();
        let block = compile_ir_memory_dword_load_block_v2(code, entry).unwrap();
        assert_eq!(block.source_bytes(), &[0x8b, 0x03]);
        assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
        assert_eq!(
            block.site().operand().address_size,
            od_isa::OperandSize::Qword
        );
        assert_eq!(block.site().destination(), Some(0));
        assert_eq!(block.site().next_rip(), entry + 2);
        assert_eq!(
            block.compile_flavor(),
            crate::translation::CompileFlavor::S3_26cIrDwordMemoryLoadV2
        );
        assert_eq!(block.pc_map().instruction_count(), 1);
        assert_eq!(block.pc_map().boundaries().len(), 1);
        assert_eq!(
            block.pc_map().boundaries()[0].rip,
            crate::statepoints::GuestRipBoundary::Fixed(entry + 2)
        );
        assert_eq!(
            block.pc_map().code_len() as usize,
            block.code().words().len() * 4
        );
        assert!(block.pc_map().lookup(0).is_some());
        assert!(block.pc_map().lookup(block.pc_map().code_len()).is_none());
        assert!(block.pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::HelperCall
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction
        }));
        assert!(block.pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::Exit
                && range.guest_rip == crate::statepoints::GuestRipSource::Fixed(entry + 2)
        }));
        assert!(
            block
                .site()
                .matches_source_bytes(&[0x8b, 0x03], MemorySiteOperationV2::Load)
        );

        for neighbor in [
            &[0x48, 0x8b, 0x03][..],
            &[0x66, 0x8b, 0x03][..],
            &[0x8b, 0x04, 0x23][..],
            &[0x8b][..],
            &[0x8b, 0x03, 0x90][..],
        ] {
            let candidate = CodeView::new(entry, neighbor).unwrap();
            assert!(
                compile_ir_memory_dword_load_block_v2(candidate, entry).is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }

        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&[0x8b, 0x03], MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.destination = Some(1);
        assert!(!forged.matches_source_bytes(&[0x8b, 0x03], MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Dword;
        assert!(!forged.matches_source_bytes(&[0x8b, 0x03], MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&[0x8b, 0x03], MemorySiteOperationV2::Load));
        assert!(
            !block
                .site()
                .matches_source_bytes(&[0x8b, 0x0b], MemorySiteOperationV2::Load)
        );
    }

    #[test]
    fn ir_dword_addr32_load_admits_only_67_8b_03_and_preserves_its_provenance() {
        let entry = 0x80f0;
        let source = [0x67, 0x8b, 0x03];
        let block =
            compile_ir_memory_dword_load_block_v2(CodeView::new(entry, &source).unwrap(), entry)
                .unwrap();
        assert_eq!(block.source_bytes(), source);
        assert_eq!(block.site().guest_rip(), entry);
        assert_eq!(block.site().next_rip(), entry + 3);
        assert_eq!(block.site().destination(), Some(0));
        assert_eq!(block.site().source(), None);
        assert_eq!(
            block.site().operand().base,
            AddressBase::Register(Register::Rbx)
        );
        assert_eq!(
            block.site().operand().address_size,
            od_isa::OperandSize::Dword
        );
        assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
        assert_eq!(block.site().operand().displacement, 0);
        assert!(
            block
                .site()
                .matches_source_bytes(&source, MemorySiteOperationV2::Load)
        );

        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.destination = Some(1);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.guest_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.next_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));

        for neighbor in [
            &[0x67, 0x8b, 0x03, 0x90][..], // trailing byte
            &[0x67, 0x67, 0x8b, 0x03][..], // repeated address override
            &[0x66, 0x67, 0x8b, 0x03][..], // reordered second prefix
            &[0x67, 0x66, 0x8b, 0x03][..], // extra operand-size prefix
            &[0x67, 0x8b][..],             // truncated
            &[0x67, 0x8b, 0x04, 0x23][..], // SIB
            &[0x67, 0x8b, 0x0b][..],       // ECX destination
            &[0x67, 0x48, 0x8b, 0x03][..], // REX prefix
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_addr32_disp8_load_binds_all_signed_source_bytes_to_its_site() {
        let entry = 0x80e0;
        let mut sites = Vec::with_capacity(256);
        for raw_displacement in 0_u16..=255 {
            let displacement = raw_displacement as u8;
            let source = [0x67, 0x8b, 0x43, displacement];
            let block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().guest_rip(), entry);
            assert_eq!(block.site().next_rip(), entry + 4);
            assert_eq!(block.site().destination(), Some(0));
            assert_eq!(block.site().source(), None);
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Dword
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(
                block.site().operand().displacement,
                i64::from(displacement as i8)
            );
            assert!(block.site().has_supported_profile());
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Load)
            );
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 4)
            );
            assert!(matches_dword_pc_map_contract(
                block.pc_map(),
                entry,
                entry + 4,
                0,
                1,
                crate::statepoints::StatePhase::HelperCall,
                MemorySiteOperationV2::Load,
            ));
            assert_ne!(
                sites.last(),
                Some(block.site()),
                "disp8 {displacement:#04x}"
            );
            sites.push(*block.site());

            let changed = [0x67, 0x8b, 0x43, displacement.wrapping_add(1)];
            assert!(
                !block
                    .site()
                    .matches_source_bytes(&changed, MemorySiteOperationV2::Load)
            );
            let changed_block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &changed).unwrap(),
                entry,
            )
            .unwrap();
            assert_ne!(block.site(), changed_block.site());
        }
        assert_eq!(sites.len(), 256);

        let source = [0x67, 0x8b, 0x43, 0x80];
        let block =
            compile_ir_memory_dword_load_block_v2(CodeView::new(entry, &source).unwrap(), entry)
                .unwrap();
        let mut forged = *block.site();
        forged.instruction_index += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.destination = Some(1);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.displacement = 128;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.next_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));

        for neighbor in [
            &[0x67, 0x8b, 0x43][..],             // truncated displacement
            &[0x67, 0x8b, 0x43, 0x00, 0x90][..], // trailing byte
            &[0x67, 0x67, 0x8b, 0x43, 0x00][..], // repeated address prefix
            &[0x66, 0x67, 0x8b, 0x43, 0x00][..], // reordered second prefix
            &[0x67, 0x66, 0x8b, 0x43, 0x00][..], // operand-size prefix
            &[0x67, 0x8b, 0x04, 0x23][..],       // SIB
            &[0x67, 0x8b, 0x4b, 0x00][..],       // ECX destination
            &[0x67, 0x48, 0x8b, 0x43, 0x00][..], // REX prefix
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_addr32_disp32_load_binds_all_source_bytes_to_its_site() {
        let entry = 0x80d0;
        let displacements = [i32::MIN, -129, -1, 0, 1, 128, i32::MAX];
        let mut sites = Vec::with_capacity(displacements.len());
        for displacement in displacements {
            let encoded_displacement = displacement.to_le_bytes();
            let source = [
                0x67,
                0x8b,
                0x83,
                encoded_displacement[0],
                encoded_displacement[1],
                encoded_displacement[2],
                encoded_displacement[3],
            ];
            let block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().guest_rip(), entry);
            assert_eq!(block.site().next_rip(), entry + 7);
            assert_eq!(block.site().destination(), Some(0));
            assert_eq!(block.site().source(), None);
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Dword
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(block.site().operand().displacement, i64::from(displacement));
            assert!(block.site().has_supported_profile());
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Load)
            );
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 7)
            );
            assert!(matches_dword_pc_map_contract(
                block.pc_map(),
                entry,
                entry + 7,
                0,
                1,
                crate::statepoints::StatePhase::HelperCall,
                MemorySiteOperationV2::Load,
            ));
            assert_ne!(sites.last(), Some(block.site()), "disp32 {displacement}");
            sites.push(*block.site());

            for source_offset in 3..7 {
                let mut changed = source;
                changed[source_offset] ^= 0x01;
                assert!(
                    !block
                        .site()
                        .matches_source_bytes(&changed, MemorySiteOperationV2::Load)
                );
                let changed_block = compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, &changed).unwrap(),
                    entry,
                )
                .unwrap();
                assert_ne!(
                    block.site(),
                    changed_block.site(),
                    "disp32 source byte {source_offset}"
                );
            }
        }

        let source = [0x67, 0x8b, 0x83, 0xff, 0xff, 0xff, 0xff];
        let block =
            compile_ir_memory_dword_load_block_v2(CodeView::new(entry, &source).unwrap(), entry)
                .unwrap();
        let mut forged = *block.site();
        forged.instruction_index += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.destination = Some(1);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.guest_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.operand.displacement = 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        let mut forged = *block.site();
        forged.next_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));

        for window in 1..source.len() {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, &source[..window]).unwrap(),
                    entry
                )
                .is_err()
            );
        }
        for neighbor in [
            &[0x67, 0x8b, 0x83, 0, 0, 0, 0, 0x90][..], // trailing byte
            &[0x67, 0x67, 0x8b, 0x83, 0, 0, 0, 0][..], // repeated address override
            &[0x66, 0x67, 0x8b, 0x83, 0, 0, 0, 0][..], // reordered prefix
            &[0x67, 0x66, 0x8b, 0x83, 0, 0, 0, 0][..], // operand-size prefix
            &[0x67, 0x8b, 0x43, 0, 0, 0, 0][..],       // disp8
            &[0x67, 0x8b, 0x84, 0, 0, 0, 0, 0][..],    // SIB
            &[0x67, 0x8b, 0x8b, 0, 0, 0, 0][..],       // ECX destination
            &[0x67, 0x48, 0x8b, 0x83, 0, 0, 0, 0][..], // REX prefix
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
        assert_eq!(sites.len(), displacements.len());
    }

    #[test]
    fn ir_dword_alias_load_admits_only_exact_8b00_and_preserves_its_provenance() {
        let entry = 0x8150;
        let alias = compile_ir_memory_dword_load_block_v2(
            CodeView::new(entry, &[0x8b, 0x00]).unwrap(),
            entry,
        )
        .unwrap();
        assert_eq!(alias.source_bytes(), &[0x8b, 0x00]);
        assert_eq!(alias.site().destination(), Some(0));
        assert_eq!(
            alias.site().operand().base,
            AddressBase::Register(Register::Rax)
        );
        assert_eq!(alias.site().next_rip(), entry + 2);
        assert_eq!(
            alias.compile_flavor(),
            crate::translation::CompileFlavor::S3_26cIrDwordMemoryLoadV2
        );
        assert_eq!(alias.pc_map().instruction_count(), 1);
        assert!(
            alias
                .site()
                .matches_source_bytes(&[0x8b, 0x00], MemorySiteOperationV2::Load)
        );
        for neighbor in [
            &[0x8b, 0x01][..],
            &[0x8b, 0x08][..],
            &[0x8b, 0x40, 0x00][..],
            &[0x8b, 0x04, 0x00][..],
            &[0x66, 0x8b, 0x00][..],
            &[0x48, 0x8b, 0x00][..],
            &[0x8b, 0x00, 0x90][..],
            &[0x8b][..],
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
            assert!(
                !alias
                    .site()
                    .matches_source_bytes(neighbor, MemorySiteOperationV2::Load),
                "descriptor accepted neighboring encoding {neighbor:02x?}"
            );
        }
        let mut forged = *alias.site();
        forged.operand.base = AddressBase::Register(Register::Rbx);
        assert!(!forged.matches_source_bytes(&[0x8b, 0x00], MemorySiteOperationV2::Load));
        let mut forged = *alias.site();
        forged.destination = Some(1);
        assert!(!forged.matches_source_bytes(&[0x8b, 0x00], MemorySiteOperationV2::Load));
        let mut forged = *alias.site();
        forged.operand.address_size = od_isa::OperandSize::Dword;
        assert!(!forged.matches_source_bytes(&[0x8b, 0x00], MemorySiteOperationV2::Load));
        let mut forged = *alias.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&[0x8b, 0x00], MemorySiteOperationV2::Load));
        assert!(
            !alias
                .site()
                .matches_source_bytes(&[0x8b, 0x03], MemorySiteOperationV2::Load)
        );
    }

    #[test]
    fn ir_dword_rbx_disp8_load_binds_all_signed_source_bytes_to_its_site() {
        let entry = 0x8180;
        let mut sites = Vec::with_capacity(256);
        for raw_displacement in 0_u16..=255 {
            let displacement = raw_displacement as u8;
            let source = [0x8b, 0x43, displacement];
            let block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            let signed = i64::from(displacement as i8);
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().operand().displacement, signed);
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(block.site().next_rip(), entry + 3);
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Load)
            );
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 3)
            );
            assert_eq!(
                block.compile_flavor(),
                crate::translation::CompileFlavor::S3_26cIrDwordMemoryLoadV2
            );
            if let Some(previous) = sites.last() {
                assert_ne!(previous, block.site(), "disp8 {displacement:#04x}");
            }
            sites.push(*block.site());

            let changed = [0x8b, 0x43, displacement.wrapping_add(1)];
            assert!(
                !block
                    .site()
                    .matches_source_bytes(&changed, MemorySiteOperationV2::Load)
            );
            let changed_block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &changed).unwrap(),
                entry,
            )
            .unwrap();
            assert_ne!(block.site(), changed_block.site());
        }
        assert_eq!(sites.len(), 256);

        for neighbor in [
            &[0x8b, 0x42, 0x00][..],
            &[0x8b, 0x44, 0x00][..],
            &[0x8b, 0x43][..],
            &[0x8b, 0x43, 0x00, 0x90][..],
            &[0x48, 0x8b, 0x43, 0x00][..],
            &[0x66, 0x8b, 0x43, 0x00][..],
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry,
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_rbx_disp32_load_binds_signed_source_bytes_to_its_site() {
        let entry = 0x81c0;
        let displacements = [
            i32::MIN,
            -129,
            -128,
            -1,
            0,
            1,
            127,
            128,
            i32::MAX,
            0x0102_0304,
            0x1020_3040,
            0x7654_3210,
        ];
        let mut sites = Vec::with_capacity(displacements.len());

        for displacement in displacements {
            let mut source = [0x8b, 0x83, 0, 0, 0, 0];
            source[2..].copy_from_slice(&displacement.to_le_bytes());
            let block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().operand().displacement, i64::from(displacement));
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Qword
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(block.site().next_rip(), entry + 6);
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Load)
            );
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 6)
            );
            assert_eq!(
                block.compile_flavor(),
                crate::translation::CompileFlavor::S3_26cIrDwordMemoryLoadV2
            );
            if let Some(previous) = sites.last() {
                assert_ne!(previous, block.site());
            }
            sites.push(*block.site());

            for byte_index in 2..6 {
                let mut changed = source;
                changed[byte_index] ^= 1;
                assert!(
                    !block
                        .site()
                        .matches_source_bytes(&changed, MemorySiteOperationV2::Load)
                );
                let changed_block = compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, &changed).unwrap(),
                    entry,
                )
                .unwrap();
                assert_ne!(block.site(), changed_block.site());
            }

            let mut forged = *block.site();
            forged.operand.base = AddressBase::Register(Register::Rax);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.operand.size = od_isa::OperandSize::Qword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.operand.address_size = od_isa::OperandSize::Dword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.guest_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.next_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        }

        assert_eq!(sites.len(), displacements.len());
        for neighbor in [
            &[0x8b, 0x82, 0, 0, 0, 0][..],
            &[0x8b, 0x84, 0x00, 0, 0, 0, 0][..],
            &[0x8b, 0x83, 0, 0, 0][..],
            &[0x8b, 0x83, 0, 0, 0, 0, 0x90][..],
            &[0x8b, 0xa3, 0, 0, 0, 0][..],
            &[0x48, 0x8b, 0x83, 0, 0, 0, 0][..],
            &[0x66, 0x8b, 0x83, 0, 0, 0, 0][..],
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_rip_relative_disp32_load_binds_all_source_bytes_to_next_rip() {
        let entry = 0x8200;
        let displacements = [
            i32::MIN,
            -129,
            -128,
            -1,
            0,
            1,
            127,
            128,
            i32::MAX,
            0x0102_0304,
            0x7f00_ff80,
        ];
        for displacement in displacements {
            let encoded = displacement.to_le_bytes();
            let source = [0x8b, 0x05, encoded[0], encoded[1], encoded[2], encoded[3]];
            let block = compile_ir_memory_dword_load_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().instruction_index(), 0);
            assert_eq!(block.site().destination(), Some(0));
            assert_eq!(block.site().source(), None);
            assert_eq!(block.site().operand().base, AddressBase::NextRip);
            assert_eq!(block.site().operand().index, None);
            assert_eq!(block.site().operand().scale, 1);
            assert_eq!(block.site().operand().displacement, i64::from(displacement));
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Qword
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(block.site().next_rip(), entry + 6);
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Load)
            );
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 6)
            );

            for byte_index in 2..6 {
                let mut changed = source;
                changed[byte_index] ^= 1;
                assert!(
                    !block
                        .site()
                        .matches_source_bytes(&changed, MemorySiteOperationV2::Load)
                );
                let changed_block = compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, &changed).unwrap(),
                    entry,
                )
                .unwrap();
                assert_ne!(block.site(), changed_block.site());
            }

            let mut forged = *block.site();
            forged.operand.base = AddressBase::Register(Register::Rbx);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.destination = Some(1);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.source = Some(0);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.operand.size = od_isa::OperandSize::Qword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.operand.address_size = od_isa::OperandSize::Dword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.guest_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
            let mut forged = *block.site();
            forged.next_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        }

        for neighbor in [
            &[0x8b, 0x05][..],
            &[0x8b, 0x05, 0, 0, 0][..],
            &[0x8b, 0x05, 0, 0, 0, 0, 0x90][..],
            &[0x8b, 0x0d, 0, 0, 0, 0][..],
            &[0x8b, 0x04, 0x25, 0, 0, 0, 0][..],
            &[0x48, 0x8b, 0x05, 0, 0, 0, 0][..],
            &[0x67, 0x8b, 0x05, 0, 0, 0, 0][..],
            &[0x66, 0x8b, 0x05, 0, 0, 0, 0][..],
        ] {
            assert!(
                compile_ir_memory_dword_load_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_store_admits_only_exact_8903_and_preserves_its_provenance() {
        let entry = 0x8200;
        let code = CodeView::new(entry, &[0x89, 0x03]).unwrap();
        let block = compile_ir_memory_dword_store_block_v2(code, entry).unwrap();
        assert_eq!(block.source_bytes(), &[0x89, 0x03]);
        assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
        assert_eq!(
            block.site().operand().address_size,
            od_isa::OperandSize::Qword
        );
        assert_eq!(block.site().source(), Some(0));
        assert_eq!(block.site().next_rip(), entry + 2);
        assert_eq!(
            block.compile_flavor(),
            crate::translation::CompileFlavor::S3_26eIrDwordMemoryStoreV2
        );
        assert_eq!(block.pc_map().instruction_count(), 1);
        assert_eq!(block.pc_map().boundaries().len(), 1);
        assert_eq!(
            block.pc_map().boundaries()[0].rip,
            crate::statepoints::GuestRipBoundary::Fixed(entry + 2)
        );
        assert!(block.pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::HelperCall
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
        }));
        assert!(block.pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::AfterHelper
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
        }));
        assert!(
            block
                .site()
                .matches_source_bytes(&[0x89, 0x03], MemorySiteOperationV2::Store)
        );

        for neighbor in [
            &[0x48, 0x89, 0x03][..],
            &[0x66, 0x89, 0x03][..],
            &[0x89, 0x04, 0x23][..],
            &[0x89][..],
            &[0x89, 0x03, 0x90][..],
        ] {
            let candidate = CodeView::new(entry, neighbor).unwrap();
            assert!(
                compile_ir_memory_dword_store_block_v2(candidate, entry).is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }

        let mut forged = *block.site();
        forged.source = Some(1);
        assert!(!forged.matches_source_bytes(&[0x89, 0x03], MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&[0x89, 0x03], MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Dword;
        assert!(!forged.matches_source_bytes(&[0x89, 0x03], MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&[0x89, 0x03], MemorySiteOperationV2::Store));
        assert!(
            !block
                .site()
                .matches_source_bytes(&[0x89, 0x0b], MemorySiteOperationV2::Store)
        );
    }

    #[test]
    fn ir_dword_addr32_store_admits_only_67_89_03_and_preserves_its_provenance() {
        let entry = 0x8210;
        let source = [0x67, 0x89, 0x03];
        let block =
            compile_ir_memory_dword_store_block_v2(CodeView::new(entry, &source).unwrap(), entry)
                .unwrap();
        assert_eq!(block.source_bytes(), source);
        assert_eq!(block.site().guest_rip(), entry);
        assert_eq!(block.site().next_rip(), entry + 3);
        assert_eq!(block.site().source(), Some(0));
        assert_eq!(block.site().destination(), None);
        assert_eq!(
            block.site().operand().base,
            AddressBase::Register(Register::Rbx)
        );
        assert_eq!(
            block.site().operand().address_size,
            od_isa::OperandSize::Dword
        );
        assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
        assert_eq!(block.site().operand().displacement, 0);
        assert!(
            block
                .site()
                .matches_source_bytes(&source, MemorySiteOperationV2::Store)
        );
        assert_eq!(block.pc_map().instruction_count(), 1);
        assert_eq!(
            block.pc_map().boundaries()[0].rip,
            crate::statepoints::GuestRipBoundary::Fixed(entry + 3)
        );
        assert!(matches_dword_pc_map_contract(
            block.pc_map(),
            entry,
            entry + 3,
            0,
            1,
            crate::statepoints::StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));
        assert!(block.pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::HelperCall
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
        }));
        assert!(block.pc_map().ranges().iter().any(|range| {
            range.phase == crate::statepoints::StatePhase::AfterHelper
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
        }));

        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.source = Some(1);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.guest_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.next_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));

        for neighbor in [
            &[0x67, 0x89, 0x03, 0x90][..],       // trailing byte
            &[0x67, 0x67, 0x89, 0x03][..],       // repeated address override
            &[0x66, 0x67, 0x89, 0x03][..],       // other prefix before address override
            &[0x67, 0x66, 0x89, 0x03][..],       // operand-size prefix
            &[0x67, 0x89][..],                   // truncated
            &[0x67, 0x89, 0x83, 0, 0, 0, 0][..], // disp32
            &[0x67, 0x89, 0x04, 0x23][..],       // SIB
            &[0x67, 0x89, 0x0b][..],             // ECX source
            &[0x67, 0x48, 0x89, 0x03][..],       // REX prefix
        ] {
            assert!(
                compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_addr32_disp8_store_binds_all_signed_source_bytes_to_its_site() {
        let entry = 0x8200;
        let mut sites = Vec::with_capacity(256);
        for raw_displacement in 0_u16..=255 {
            let displacement = raw_displacement as u8;
            let source = [0x67, 0x89, 0x43, displacement];
            let block = compile_ir_memory_dword_store_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().guest_rip(), entry);
            assert_eq!(block.site().next_rip(), entry + 4);
            assert_eq!(block.site().source(), Some(0));
            assert_eq!(block.site().destination(), None);
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Dword
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(
                block.site().operand().displacement,
                i64::from(displacement as i8)
            );
            assert!(block.site().has_supported_profile());
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Store)
            );
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 4)
            );
            assert!(matches_dword_pc_map_contract(
                block.pc_map(),
                entry,
                entry + 4,
                0,
                1,
                crate::statepoints::StatePhase::HelperCall,
                MemorySiteOperationV2::Store,
            ));
            assert!(block.pc_map().ranges().iter().any(|range| {
                range.phase == crate::statepoints::StatePhase::HelperCall
                    && range.snapshot
                        == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
            }));
            assert!(block.pc_map().ranges().iter().any(|range| {
                range.phase == crate::statepoints::StatePhase::AfterHelper
                    && range.snapshot
                        == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
            }));
            assert_ne!(
                sites.last(),
                Some(block.site()),
                "disp8 {displacement:#04x}"
            );
            sites.push(*block.site());

            let changed = [0x67, 0x89, 0x43, displacement.wrapping_add(1)];
            assert!(
                !block
                    .site()
                    .matches_source_bytes(&changed, MemorySiteOperationV2::Store)
            );
            let changed_block = compile_ir_memory_dword_store_block_v2(
                CodeView::new(entry, &changed).unwrap(),
                entry,
            )
            .unwrap();
            assert_ne!(block.site(), changed_block.site());
        }

        let source = [0x67, 0x89, 0x43, 0x80];
        let block =
            compile_ir_memory_dword_store_block_v2(CodeView::new(entry, &source).unwrap(), entry)
                .unwrap();
        let mut forged = *block.site();
        forged.instruction_index += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.base = AddressBase::Register(Register::Rax);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.address_size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.source = Some(1);
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.guest_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.next_rip += 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        let mut forged = *block.site();
        forged.operand.displacement = 1;
        assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        assert!(
            !block
                .site()
                .matches_source_bytes(&[0x67, 0x89, 0x03], MemorySiteOperationV2::Store)
        );

        for neighbor in [
            &[0x67, 0x89, 0x43][..],             // truncated displacement
            &[0x67, 0x89, 0x43, 0x00, 0x90][..], // trailing byte
            &[0x67, 0x67, 0x89, 0x43, 0x00][..], // repeated address override
            &[0x66, 0x67, 0x89, 0x43, 0x00][..], // prefix before address override
            &[0x67, 0x66, 0x89, 0x43, 0x00][..], // operand-size prefix
            &[0x67, 0x89, 0x83, 0, 0, 0, 0][..], // disp32
            &[0x67, 0x89, 0x44, 0x23, 0][..],    // SIB
            &[0x67, 0x89, 0x4b, 0][..],          // ECX source
            &[0x67, 0x48, 0x89, 0x43, 0][..],    // REX prefix
        ] {
            assert!(
                compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
        assert_eq!(sites.len(), 256);
    }

    #[test]
    fn ir_dword_alias_store_admits_only_exact_8900_and_preserves_its_provenance() {
        let entry = 0x8250;
        let alias = compile_ir_memory_dword_store_block_v2(
            CodeView::new(entry, &[0x89, 0x00]).unwrap(),
            entry,
        )
        .unwrap();
        assert_eq!(alias.source_bytes(), &[0x89, 0x00]);
        assert_eq!(alias.site().source(), Some(0));
        assert_eq!(
            alias.site().operand().base,
            AddressBase::Register(Register::Rax)
        );
        assert_eq!(alias.site().operand().size, od_isa::OperandSize::Dword);
        assert_eq!(alias.site().next_rip(), entry + 2);
        assert!(
            alias
                .site()
                .matches_source_bytes(&[0x89, 0x00], MemorySiteOperationV2::Store)
        );
        assert!(
            !alias
                .site()
                .matches_source_bytes(&[0x89, 0x03], MemorySiteOperationV2::Store)
        );

        for neighbor in [
            &[0x89, 0x01][..],
            &[0x89, 0x02][..],
            &[0x89, 0x04, 0x00][..],
            &[0x89, 0x40, 0x00][..],
            &[0x48, 0x89, 0x00][..],
            &[0x67, 0x89, 0x00][..],
            &[0x89][..],
            &[0x89, 0x00, 0x90][..],
        ] {
            let candidate = CodeView::new(entry, neighbor).unwrap();
            assert!(
                compile_ir_memory_dword_store_block_v2(candidate, entry).is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }

        let mut forged = *alias.site();
        forged.source = Some(1);
        assert!(!forged.matches_source_bytes(&[0x89, 0x00], MemorySiteOperationV2::Store));
        let mut forged = *alias.site();
        forged.operand.base = AddressBase::Register(Register::Rbx);
        assert!(!forged.matches_source_bytes(&[0x89, 0x00], MemorySiteOperationV2::Store));
        let mut forged = *alias.site();
        forged.operand.size = od_isa::OperandSize::Qword;
        assert!(!forged.matches_source_bytes(&[0x89, 0x00], MemorySiteOperationV2::Store));
    }

    #[test]
    fn ir_dword_rbx_disp8_store_binds_all_signed_source_bytes_to_its_site() {
        let entry = 0x8280;
        let mut sites = Vec::with_capacity(256);
        for raw_displacement in 0_u16..=255 {
            let displacement = raw_displacement as u8;
            let source = [0x89, 0x43, displacement];
            let block = compile_ir_memory_dword_store_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().source(), Some(0));
            assert_eq!(block.site().destination(), None);
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Qword
            );
            assert_eq!(
                block.site().operand().displacement,
                i64::from(displacement as i8)
            );
            assert_eq!(block.site().next_rip(), entry + 3);
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 3)
            );
            assert_eq!(
                block.compile_flavor(),
                crate::translation::CompileFlavor::S3_26eIrDwordMemoryStoreV2
            );
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Store)
            );
            if let Some(previous) = sites.last() {
                assert_ne!(previous, block.site(), "disp8 {displacement:#04x}");
            }
            sites.push(*block.site());

            let changed = [0x89, 0x43, displacement.wrapping_add(1)];
            assert!(
                !block
                    .site()
                    .matches_source_bytes(&changed, MemorySiteOperationV2::Store)
            );
            let changed_block = compile_ir_memory_dword_store_block_v2(
                CodeView::new(entry, &changed).unwrap(),
                entry,
            )
            .unwrap();
            assert_ne!(block.site(), changed_block.site());

            let mut forged = *block.site();
            forged.source = Some(1);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.base = AddressBase::Register(Register::Rax);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.size = od_isa::OperandSize::Qword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.address_size = od_isa::OperandSize::Dword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.guest_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.next_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        }
        assert_eq!(sites.len(), 256);

        for neighbor in [
            &[0x89, 0x42, 0x00][..],
            &[0x89, 0x44, 0x00][..],
            &[0x89, 0x43][..],
            &[0x89, 0x43, 0x00, 0x90][..],
            &[0x89, 0x4b, 0x00][..],
            &[0x48, 0x89, 0x43, 0x00][..],
            &[0x66, 0x89, 0x43, 0x00][..],
        ] {
            assert!(
                compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_rbx_disp32_store_binds_all_source_bytes_to_its_site() {
        let entry = 0x82c0;
        let displacements = [
            i32::MIN,
            -129,
            -128,
            -2,
            -1,
            0,
            1,
            127,
            128,
            0x0102_0304,
            0x7f00_ff80,
            i32::MAX,
        ];
        for displacement in displacements {
            let encoded = displacement.to_le_bytes();
            let source = [0x89, 0x83, encoded[0], encoded[1], encoded[2], encoded[3]];
            let block = compile_ir_memory_dword_store_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().instruction_index(), 0);
            assert_eq!(block.site().source(), Some(0));
            assert_eq!(block.site().destination(), None);
            assert_eq!(
                block.site().operand().base,
                AddressBase::Register(Register::Rbx)
            );
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Qword
            );
            assert_eq!(block.site().operand().displacement, i64::from(displacement));
            assert_eq!(block.site().next_rip(), entry + 6);
            assert_eq!(block.pc_map().instruction_count(), 1);
            assert_eq!(
                block.pc_map().boundaries()[0].rip,
                crate::statepoints::GuestRipBoundary::Fixed(entry + 6)
            );
            assert_eq!(
                block.compile_flavor(),
                crate::translation::CompileFlavor::S3_26eIrDwordMemoryStoreV2
            );
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Store)
            );

            for byte_index in 2..6 {
                let mut changed = source;
                changed[byte_index] = changed[byte_index].wrapping_add(1);
                assert!(
                    !block
                        .site()
                        .matches_source_bytes(&changed, MemorySiteOperationV2::Store),
                    "disp32 byte {byte_index} remained bound for {displacement:#x}"
                );
                let changed_block = compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, &changed).unwrap(),
                    entry,
                )
                .unwrap();
                assert_ne!(block.site(), changed_block.site());
            }

            let mut forged = *block.site();
            forged.source = Some(1);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.base = AddressBase::Register(Register::Rax);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.size = od_isa::OperandSize::Qword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.address_size = od_isa::OperandSize::Dword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.guest_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.next_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        }

        for neighbor in [
            &[0x89, 0x82, 0, 0, 0, 0][..],
            &[0x89, 0x84, 0x20, 0, 0, 0, 0][..],
            &[0x89, 0x83, 0, 0, 0][..],
            &[0x89, 0x83, 0, 0, 0, 0, 0x90][..],
            &[0x89, 0x8b, 0, 0, 0, 0][..],
            &[0x48, 0x89, 0x83, 0, 0, 0, 0][..],
            &[0x67, 0x89, 0x83, 0, 0, 0, 0][..],
            &[0x66, 0x89, 0x83, 0, 0, 0, 0][..],
        ] {
            assert!(
                compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_rip_relative_disp32_store_binds_all_source_bytes_to_next_rip() {
        let entry = 0x82e0;
        let displacements = [
            i32::MIN,
            -129,
            -128,
            -1,
            0,
            1,
            127,
            128,
            0x0102_0304,
            0x7f00_ff80,
            i32::MAX,
        ];
        for displacement in displacements {
            let encoded = displacement.to_le_bytes();
            let source = [0x89, 0x05, encoded[0], encoded[1], encoded[2], encoded[3]];
            let block = compile_ir_memory_dword_store_block_v2(
                CodeView::new(entry, &source).unwrap(),
                entry,
            )
            .unwrap();
            assert_eq!(block.source_bytes(), source);
            assert_eq!(block.site().instruction_index(), 0);
            assert_eq!(block.site().guest_rip(), entry);
            assert_eq!(block.site().next_rip(), entry + 6);
            assert_eq!(block.site().source(), Some(0));
            assert_eq!(block.site().destination(), None);
            assert_eq!(block.site().operand().base, AddressBase::NextRip);
            assert_eq!(block.site().operand().displacement, i64::from(displacement));
            assert_eq!(block.site().operand().size, od_isa::OperandSize::Dword);
            assert_eq!(
                block.site().operand().address_size,
                od_isa::OperandSize::Qword
            );
            assert!(
                block
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Store)
            );

            for byte_index in 2..6 {
                let mut changed = source;
                changed[byte_index] = changed[byte_index].wrapping_add(1);
                assert!(
                    !block
                        .site()
                        .matches_source_bytes(&changed, MemorySiteOperationV2::Store)
                );
                let changed_block = compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, &changed).unwrap(),
                    entry,
                )
                .unwrap();
                assert_ne!(block.site(), changed_block.site());
            }

            let mut forged = *block.site();
            forged.operand.base = AddressBase::Register(Register::Rbx);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.size = od_isa::OperandSize::Qword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.operand.address_size = od_isa::OperandSize::Dword;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.guest_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.next_rip += 1;
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
            let mut forged = *block.site();
            forged.source = Some(1);
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Store));
        }

        let entry = 0x82f0;
        for neighbor in [
            &[0x89, 0x04, 0x25, 0, 0, 0, 0][..], // SIB absolute disp32
            &[0x89, 0x0d, 0, 0, 0, 0][..],       // ECX source
            &[0x89, 0x05, 0, 0, 0][..],          // truncated
            &[0x89, 0x05, 0, 0, 0, 0, 0x90][..], // trailing byte
            &[0x48, 0x89, 0x05, 0, 0, 0, 0][..], // qword
            &[0x67, 0x89, 0x05, 0, 0, 0, 0][..], // address-size override
            &[0x66, 0x89, 0x05, 0, 0, 0, 0][..], // operand-size override
            &[0xf3, 0x89, 0x05, 0, 0, 0, 0][..], // group-1 prefix
        ] {
            assert!(
                compile_ir_memory_dword_store_block_v2(
                    CodeView::new(entry, neighbor).unwrap(),
                    entry
                )
                .is_err(),
                "admitted neighboring encoding {neighbor:02x?}"
            );
        }
    }

    #[test]
    fn ir_dword_load_pc_map_has_exact_8b03_single_site_contract() {
        let entry = 0x8300;
        let block = compile_ir_memory_dword_load_block_v2(
            CodeView::new(entry, &[0x8b, 0x03]).unwrap(),
            entry,
        )
        .unwrap();
        let map = block.pc_map();
        assert!(matches_dword_pc_map_contract(
            map,
            entry,
            entry + 2,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Load,
        ));

        for (site_rip, next_rip, site_index, instruction_count, helper_phase) in [
            (entry + 1, entry + 2, 0, 1, StatePhase::HelperCall),
            (entry, entry + 3, 0, 1, StatePhase::HelperCall),
            (entry, entry + 2, 1, 1, StatePhase::HelperCall),
            (entry, entry + 2, 0, 2, StatePhase::HelperCall),
            (entry, entry + 2, 0, 1, StatePhase::AfterHelper),
        ] {
            assert!(
                !matches_dword_pc_map_contract(
                    map,
                    site_rip,
                    next_rip,
                    site_index,
                    instruction_count,
                    helper_phase,
                    MemorySiteOperationV2::Load,
                ),
                "accepted mutated load PC-map expectation: site={site_rip:#x}, next={next_rip:#x}, index={site_index}, count={instruction_count}, phase={helper_phase:?}"
            );
        }
    }

    #[test]
    fn ir_dword_addr32_load_pc_map_uses_three_byte_guest_boundary() {
        let entry = 0x8320;
        let block = compile_ir_memory_dword_load_block_v2(
            CodeView::new(entry, &[0x67, 0x8b, 0x03]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            block.pc_map(),
            entry,
            entry + 3,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Load,
        ));
        for (next_rip, phase) in [
            (entry + 2, StatePhase::HelperCall),
            (entry + 4, StatePhase::HelperCall),
            (entry + 3, StatePhase::AfterHelper),
        ] {
            assert!(!matches_dword_pc_map_contract(
                block.pc_map(),
                entry,
                next_rip,
                0,
                1,
                phase,
                MemorySiteOperationV2::Load,
            ));
        }
    }

    #[test]
    fn ir_dword_alias_load_pc_map_has_exact_8b00_single_site_contract() {
        let entry = 0x8350;
        let alias = compile_ir_memory_dword_load_block_v2(
            CodeView::new(entry, &[0x8b, 0x00]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            alias.pc_map(),
            entry,
            entry + 2,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Load,
        ));
    }

    #[test]
    fn ir_dword_rbx_disp32_load_pc_map_uses_six_byte_guest_boundary() {
        let entry = 0x8380;
        let block = compile_ir_memory_dword_load_block_v2(
            CodeView::new(entry, &[0x8b, 0x83, 0x78, 0x56, 0x34, 0x12]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            block.pc_map(),
            entry,
            entry + 6,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Load,
        ));
        for rejected_next_rip in [entry + 2, entry + 3, entry + 7] {
            assert!(
                !matches_dword_pc_map_contract(
                    block.pc_map(),
                    entry,
                    rejected_next_rip,
                    0,
                    1,
                    StatePhase::HelperCall,
                    MemorySiteOperationV2::Load,
                ),
                "accepted mutated dword load PC-map next RIP {rejected_next_rip:#x}"
            );
        }
    }

    #[test]
    fn ir_dword_rip_relative_disp32_load_pc_map_uses_six_byte_guest_boundary() {
        let entry = 0x8390;
        let block = compile_ir_memory_dword_load_block_v2(
            CodeView::new(entry, &[0x8b, 0x05, 0x78, 0x56, 0x34, 0x12]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            block.pc_map(),
            entry,
            entry + 6,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Load,
        ));
        for (next_rip, phase) in [
            (entry + 2, StatePhase::HelperCall),
            (entry + 3, StatePhase::HelperCall),
            (entry + 7, StatePhase::HelperCall),
            (entry + 6, StatePhase::AfterHelper),
        ] {
            assert!(
                !matches_dword_pc_map_contract(
                    block.pc_map(),
                    entry,
                    next_rip,
                    0,
                    1,
                    phase,
                    MemorySiteOperationV2::Load,
                ),
                "accepted mutated RIP-relative dword load map: next={next_rip:#x}, phase={phase:?}"
            );
        }
    }

    #[test]
    fn ir_dword_store_pc_map_has_exact_8903_single_site_contract() {
        let entry = 0x8400;
        let block = compile_ir_memory_dword_store_block_v2(
            CodeView::new(entry, &[0x89, 0x03]).unwrap(),
            entry,
        )
        .unwrap();
        let map = block.pc_map();
        assert!(matches_dword_pc_map_contract(
            map,
            entry,
            entry + 2,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));

        for (site_rip, next_rip, site_index, instruction_count, helper_phase) in [
            (entry + 1, entry + 2, 0, 1, StatePhase::HelperCall),
            (entry, entry + 3, 0, 1, StatePhase::HelperCall),
            (entry, entry + 2, 1, 1, StatePhase::HelperCall),
            (entry, entry + 2, 0, 2, StatePhase::HelperCall),
            (entry, entry + 2, 0, 1, StatePhase::AfterHelper),
        ] {
            assert!(
                !matches_dword_pc_map_contract(
                    map,
                    site_rip,
                    next_rip,
                    site_index,
                    instruction_count,
                    helper_phase,
                    MemorySiteOperationV2::Store,
                ),
                "accepted mutated store PC-map expectation: site={site_rip:#x}, next={next_rip:#x}, index={site_index}, count={instruction_count}, phase={helper_phase:?}"
            );
        }

        let mut malformed_ranges = map.ranges().to_vec();
        let second_next_rip_exit = malformed_ranges
            .iter_mut()
            .filter(|range| {
                range.phase == StatePhase::Exit
                    && range.guest_rip == crate::statepoints::GuestRipSource::Fixed(entry + 2)
            })
            .nth(1)
            .expect("store has separate success and STOP exits at A+2");
        second_next_rip_exit.guest_rip = crate::statepoints::GuestRipSource::Fixed(entry + 3);
        let malformed_map = crate::statepoints::PcMap::new(
            map.code_len() as usize,
            map.instruction_count() as usize,
            malformed_ranges,
            map.boundaries().to_vec(),
        )
        .unwrap();
        assert!(!matches_dword_pc_map_contract(
            &malformed_map,
            entry,
            entry + 2,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));
    }

    #[test]
    fn ir_dword_alias_store_pc_map_has_exact_8900_single_site_contract() {
        let entry = 0x8450;
        let alias = compile_ir_memory_dword_store_block_v2(
            CodeView::new(entry, &[0x89, 0x00]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            alias.pc_map(),
            entry,
            entry + 2,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));
    }

    #[test]
    fn ir_dword_rbx_disp8_store_pc_map_uses_three_byte_guest_boundary() {
        let entry = 0x8470;
        let block = compile_ir_memory_dword_store_block_v2(
            CodeView::new(entry, &[0x89, 0x43, 0xff]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            block.pc_map(),
            entry,
            entry + 3,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));
        for (next_rip, phase) in [
            (entry + 2, StatePhase::HelperCall),
            (entry + 4, StatePhase::HelperCall),
            (entry + 3, StatePhase::AfterHelper),
        ] {
            assert!(
                !matches_dword_pc_map_contract(
                    block.pc_map(),
                    entry,
                    next_rip,
                    0,
                    1,
                    phase,
                    MemorySiteOperationV2::Store,
                ),
                "accepted mutated store PC-map expectation: next={next_rip:#x}, phase={phase:?}"
            );
        }
    }

    #[test]
    fn ir_dword_rbx_disp32_store_pc_map_uses_six_byte_guest_boundary() {
        let entry = 0x83b0;
        let block = compile_ir_memory_dword_store_block_v2(
            CodeView::new(entry, &[0x89, 0x83, 0x78, 0x56, 0x34, 0x12]).unwrap(),
            entry,
        )
        .unwrap();
        assert!(matches_dword_pc_map_contract(
            block.pc_map(),
            entry,
            entry + 6,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));
        for (next_rip, phase) in [
            (entry + 2, StatePhase::HelperCall),
            (entry + 3, StatePhase::HelperCall),
            (entry + 7, StatePhase::HelperCall),
            (entry + 6, StatePhase::AfterHelper),
        ] {
            assert!(
                !matches_dword_pc_map_contract(
                    block.pc_map(),
                    entry,
                    next_rip,
                    0,
                    1,
                    phase,
                    MemorySiteOperationV2::Store,
                ),
                "accepted mutated dword store PC-map expectation: next={next_rip:#x}, phase={phase:?}"
            );
        }
    }

    #[test]
    fn ir_dword_rip_relative_disp32_store_pc_map_uses_six_byte_guest_boundary() {
        let entry = 0x83d0;
        let block = compile_ir_memory_dword_store_block_v2(
            CodeView::new(entry, &[0x89, 0x05, 0x78, 0x56, 0x34, 0x12]).unwrap(),
            entry,
        )
        .unwrap();
        let map = block.pc_map();
        assert!(matches_dword_pc_map_contract(
            map,
            entry,
            entry + 6,
            0,
            1,
            StatePhase::HelperCall,
            MemorySiteOperationV2::Store,
        ));
        assert!(map.ranges().iter().any(|range| {
            range.phase == StatePhase::CommitInProgress
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleDuringCommit
        }));
        assert!(map.ranges().iter().any(|range| {
            range.phase == StatePhase::AfterHelper
                && range.guest_rip == crate::statepoints::GuestRipSource::Fixed(entry)
                && range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
        }));

        for (next_rip, phase) in [
            (entry + 2, StatePhase::HelperCall),
            (entry + 3, StatePhase::HelperCall),
            (entry + 7, StatePhase::HelperCall),
            (entry + 6, StatePhase::AfterHelper),
        ] {
            assert!(!matches_dword_pc_map_contract(
                map,
                entry,
                next_rip,
                0,
                1,
                phase,
                MemorySiteOperationV2::Store,
            ));
        }
    }

    #[test]
    fn ir_store_then_load_is_one_typed_two_site_entry() {
        let bytes = [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b];
        let block = compile_ir_memory_store_then_load_block_v2(
            CodeView::new(0x2000, &bytes).unwrap(),
            0x2000,
        )
        .unwrap();
        assert_eq!(
            block.compile_flavor(),
            crate::translation::CompileFlavor::S3_26dIrTwoMemoryV2
        );
        assert_eq!(block.source_bytes(), &bytes);
        assert_eq!(block.sites()[0].instruction_index(), 0);
        assert_eq!(block.sites()[0].operation(), MemorySiteOperationV2::Store);
        assert_eq!(block.sites()[0].guest_rip(), 0x2000);
        assert_eq!(block.sites()[1].instruction_index(), 1);
        assert_eq!(block.sites()[1].operation(), MemorySiteOperationV2::Load);
        assert_eq!(block.sites()[1].guest_rip(), 0x2003);
        assert_eq!(block.pc_map().instruction_count(), 2);
        assert!(block.pc_map().ranges().iter().any(|range| range.phase
            == crate::statepoints::StatePhase::HelperCall
            && range.instruction_index == Some(0)));
        assert!(block.pc_map().ranges().iter().any(|range| range.phase
            == crate::statepoints::StatePhase::HelperCall
            && range.instruction_index == Some(1)));
        assert!(
            block
                .pc_map()
                .ranges()
                .iter()
                .filter(|range| range.phase == crate::statepoints::StatePhase::AfterHelper)
                .all(|range| range.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect)
        );
        assert_eq!(block.pc_map().boundaries().len(), 2);
        assert!(block.pc_map().ranges().iter().any(|r| {
            r.phase == crate::statepoints::StatePhase::Service
                && r.guest_rip == crate::statepoints::GuestRipSource::CanonicalState
        }));
        assert!(
            block
                .pc_map()
                .ranges()
                .iter()
                .any(|r| r.phase == crate::statepoints::StatePhase::HelperCall
                    && r.instruction_index == Some(0))
        );
        assert!(
            block
                .pc_map()
                .ranges()
                .iter()
                .any(|r| r.phase == crate::statepoints::StatePhase::HelperCall
                    && r.instruction_index == Some(1))
        );
        assert!(block.pc_map().ranges().iter().any(|r| {
            r.phase == crate::statepoints::StatePhase::Exit
                && r.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x2000)
                && r.instruction_index == Some(0)
                && r.snapshot == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction
        }));
        assert!(block.pc_map().ranges().iter().any(|r| {
            r.phase == crate::statepoints::StatePhase::Exit
                && r.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x2003)
                && r.instruction_index == Some(1)
                && r.snapshot == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction
        }));
        assert!(
            block
                .pc_map()
                .ranges()
                .iter()
                .filter(|r| r.phase == crate::statepoints::StatePhase::AfterHelper)
                .all(|r| r.snapshot
                    == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect)
        );
        assert!(
            compile_ir_memory_store_then_load_block_v2(
                CodeView::new(0x2000, &[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0f]).unwrap(),
                0x2000,
            )
            .is_err()
        );
    }

    #[test]
    fn ir_load_then_store_is_one_typed_two_site_entry() {
        let bytes = [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02];
        let block =
            compile_ir_memory_two_site_block_v2(CodeView::new(0x3000, &bytes).unwrap(), 0x3000)
                .unwrap();
        assert_eq!(
            block.sequence(),
            crate::runtime::MemoryPairSequenceV2::LoadThenStore
        );
        assert_eq!(block.source_bytes(), &bytes);
        assert_eq!(block.sites()[0].instruction_index(), 0);
        assert_eq!(block.sites()[0].operation(), MemorySiteOperationV2::Load);
        assert_eq!(block.sites()[0].destination(), Some(0));
        assert_eq!(
            block.sites()[0].operand().base,
            AddressBase::Register(Register::Rbx)
        );
        assert_eq!(block.sites()[0].guest_rip(), 0x3000);
        assert_eq!(block.sites()[0].next_rip(), 0x3003);
        assert_eq!(block.sites()[1].instruction_index(), 1);
        assert_eq!(block.sites()[1].operation(), MemorySiteOperationV2::Store);
        assert_eq!(block.sites()[1].source(), Some(0));
        assert_eq!(
            block.sites()[1].operand().base,
            AddressBase::Register(Register::Rdx)
        );
        assert_eq!(block.sites()[1].guest_rip(), 0x3003);
        assert_eq!(block.sites()[1].next_rip(), 0x3006);
        assert_eq!(block.pc_map().instruction_count(), 2);
    }

    #[test]
    fn ir_load_then_load_is_one_typed_two_site_entry() {
        let bytes = [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b];
        let block =
            compile_ir_memory_two_site_block_v2(CodeView::new(0x5000, &bytes).unwrap(), 0x5000)
                .unwrap();
        assert_eq!(
            block.sequence(),
            crate::runtime::MemoryPairSequenceV2::LoadThenLoad
        );
        assert_eq!(block.source_bytes(), &bytes);
        assert_eq!(block.sites()[0].instruction_index(), 0);
        assert_eq!(block.sites()[0].operation(), MemorySiteOperationV2::Load);
        assert_eq!(block.sites()[0].destination(), Some(0));
        assert_eq!(block.sites()[0].guest_rip(), 0x5000);
        assert_eq!(block.sites()[0].next_rip(), 0x5003);
        assert_eq!(block.sites()[1].instruction_index(), 1);
        assert_eq!(block.sites()[1].operation(), MemorySiteOperationV2::Load);
        assert_eq!(block.sites()[1].destination(), Some(1));
        assert_eq!(block.sites()[1].guest_rip(), 0x5003);
        assert_eq!(block.sites()[1].next_rip(), 0x5006);
        assert_eq!(block.pc_map().instruction_count(), 2);
        assert_eq!(
            block
                .pc_map()
                .ranges()
                .iter()
                .filter(|range| range.phase == crate::statepoints::StatePhase::HelperCall)
                .filter_map(|range| range.instruction_index)
                .collect::<Vec<_>>(),
            [0, 1]
        );

        for unsupported in [
            [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x13], // RDX destination
            [0x48, 0x8b, 0x0b, 0x48, 0x8b, 0x03], // reversed destination order
            [0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b], // load-store alias shape
        ] {
            assert!(
                compile_ir_memory_two_site_block_v2(
                    CodeView::new(0x5000, &unsupported).unwrap(),
                    0x5000
                )
                .is_err()
            );
        }
    }

    #[test]
    fn ir_two_site_pc_maps_attribute_all_checked_ranges_and_boundaries() {
        use crate::statepoints::{GuestRipSource, SnapshotAdmissibility, StatePhase};

        let cases = [
            (
                [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b],
                crate::runtime::MemoryPairSequenceV2::StoreThenLoad,
                [MemorySiteOperationV2::Store, MemorySiteOperationV2::Load],
            ),
            (
                [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02],
                crate::runtime::MemoryPairSequenceV2::LoadThenStore,
                [MemorySiteOperationV2::Load, MemorySiteOperationV2::Store],
            ),
            (
                [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b],
                crate::runtime::MemoryPairSequenceV2::LoadThenLoad,
                [MemorySiteOperationV2::Load, MemorySiteOperationV2::Load],
            ),
        ];

        for (bytes, sequence, operations) in cases {
            let entry_rip = 0x6000;
            let block = compile_ir_memory_two_site_block_v2(
                CodeView::new(entry_rip, &bytes).unwrap(),
                entry_rip,
            )
            .unwrap();
            assert_eq!(block.sequence(), sequence);
            assert_eq!(
                [block.sites()[0].operation(), block.sites()[1].operation()],
                operations
            );
            let map = block.pc_map();
            assert_eq!(
                map.code_len() as usize,
                block.code().to_le_bytes().unwrap().len()
            );
            assert!(matches_two_site_pc_map_contract(map, entry_rip, operations));

            for range in map.ranges() {
                assert_eq!(map.lookup(range.range.start()), Some(range));
                if range.range.end() < map.code_len() {
                    let following = map.lookup(range.range.end()).unwrap();
                    assert_eq!(following.range.start(), range.range.end());
                }
            }
            for index in 0..2u32 {
                let commit = map
                    .ranges()
                    .iter()
                    .find(|range| {
                        range.instruction_index == Some(index)
                            && range.phase == StatePhase::CommitInProgress
                    })
                    .unwrap();
                assert_eq!(
                    commit.snapshot,
                    SnapshotAdmissibility::InadmissibleDuringCommit
                );
                assert_eq!(
                    commit.guest_rip,
                    GuestRipSource::Fixed(entry_rip + u64::from(index) * 3)
                );
            }
        }

        // The comparator must reject independently plausible maps with a bad
        // site, guest RIP, retirement boundary, or replay classification.
        let bytes = [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b];
        let entry_rip = 0x7000;
        let block = compile_ir_memory_two_site_block_v2(
            CodeView::new(entry_rip, &bytes).unwrap(),
            entry_rip,
        )
        .unwrap();
        let original = block.pc_map();
        let operations = [MemorySiteOperationV2::Store, MemorySiteOperationV2::Load];
        let rebuild = |ranges, boundaries| {
            crate::statepoints::PcMap::new(
                original.code_len() as usize,
                original.instruction_count() as usize,
                ranges,
                boundaries,
            )
            .unwrap()
        };

        let mut ranges = original.ranges().to_vec();
        let helper = ranges
            .iter_mut()
            .find(|range| {
                range.instruction_index == Some(0) && range.phase == StatePhase::HelperCall
            })
            .unwrap();
        helper.instruction_index = Some(1);
        assert!(!matches_two_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            operations,
        ));

        let mut ranges = original.ranges().to_vec();
        let epilogue = ranges
            .iter_mut()
            .filter(|range| range.phase == StatePhase::Service)
            .last()
            .unwrap();
        epilogue.guest_rip = GuestRipSource::Fixed(entry_rip);
        epilogue.instruction_index = Some(0);
        assert!(!matches_two_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            operations,
        ));

        let mut ranges = original.ranges().to_vec();
        let helper = ranges
            .iter_mut()
            .find(|range| {
                range.instruction_index == Some(0) && range.phase == StatePhase::HelperCall
            })
            .unwrap();
        helper.guest_rip = GuestRipSource::Fixed(entry_rip + 3);
        assert!(!matches_two_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            operations,
        ));

        let mut boundaries = original.boundaries().to_vec();
        boundaries[0].committed_instructions = 2;
        assert!(!matches_two_site_pc_map_contract(
            &rebuild(original.ranges().to_vec(), boundaries),
            entry_rip,
            operations,
        ));

        let mut ranges = original.ranges().to_vec();
        let after_store = ranges
            .iter_mut()
            .find(|range| {
                range.instruction_index == Some(0) && range.phase == StatePhase::AfterHelper
            })
            .unwrap();
        after_store.snapshot = SnapshotAdmissibility::ReplayCurrentInstruction;
        assert!(!matches_two_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            operations,
        ));
    }

    #[test]
    fn ir_single_site_pc_maps_attribute_all_checked_ranges_and_boundaries() {
        use crate::statepoints::{GuestRipSource, SnapshotAdmissibility, StatePhase};

        let cases = [
            ([0x48, 0x8b, 0x03], MemorySiteOperationV2::Load),
            ([0x48, 0x89, 0x03], MemorySiteOperationV2::Store),
        ];
        for (bytes, operation) in cases {
            let entry_rip = 0x8000;
            let code = CodeView::new(entry_rip, &bytes).unwrap();
            let (map, code_len, site) = match operation {
                MemorySiteOperationV2::Load => {
                    let block = compile_ir_memory_load_block_v2(code, entry_rip).unwrap();
                    assert_eq!(block.site().operation(), operation);
                    let map = block.pc_map().clone();
                    let code_len = block.code().to_le_bytes().unwrap().len();
                    assert!(matches_single_site_pc_map_contract(
                        &map, entry_rip, operation
                    ));
                    (map, code_len, block.site().clone())
                }
                MemorySiteOperationV2::Store => {
                    let block = compile_ir_memory_store_block_v2(code, entry_rip).unwrap();
                    assert_eq!(block.site().operation(), operation);
                    let map = block.pc_map().clone();
                    let code_len = block.code().to_le_bytes().unwrap().len();
                    assert!(matches_single_site_pc_map_contract(
                        &map, entry_rip, operation
                    ));
                    (map, code_len, block.site().clone())
                }
            };
            assert_eq!(map.code_len() as usize, code_len);
            assert_eq!(map.instruction_count(), 1);
            assert_eq!(site.instruction_index(), 0);
            assert_eq!(site.guest_rip(), entry_rip);
            assert_eq!(site.next_rip(), entry_rip + 3);
            for range in map.ranges() {
                assert_eq!(map.lookup(range.range.start()), Some(range));
                if range.range.end() < map.code_len() {
                    let next = map.lookup(range.range.end()).unwrap();
                    assert_eq!(next.range.start(), range.range.end());
                }
            }
            assert!(map.lookup(map.code_len()).is_none());
            assert!(map.snapshot_admissibility_at(map.code_len()).is_none());
            let commit = map
                .ranges()
                .iter()
                .find(|range| {
                    range.instruction_index == Some(0)
                        && range.phase == StatePhase::CommitInProgress
                })
                .unwrap();
            assert_eq!(commit.guest_rip, GuestRipSource::Fixed(entry_rip));
            assert_eq!(
                commit.snapshot,
                SnapshotAdmissibility::InadmissibleDuringCommit
            );
            let helper = map
                .ranges()
                .iter()
                .find(|range| {
                    range.instruction_index == Some(0) && range.phase == StatePhase::HelperCall
                })
                .unwrap();
            let after_helper = map
                .ranges()
                .iter()
                .find(|range| {
                    range.instruction_index == Some(0) && range.phase == StatePhase::AfterHelper
                })
                .unwrap();
            assert_eq!(helper.guest_rip, GuestRipSource::Fixed(entry_rip));
            assert_eq!(after_helper.guest_rip, GuestRipSource::Fixed(entry_rip));
            assert_eq!(
                after_helper.snapshot,
                if operation == MemorySiteOperationV2::Load {
                    SnapshotAdmissibility::ReplayCurrentInstruction
                } else {
                    SnapshotAdmissibility::InadmissibleAfterEffect
                }
            );
        }

        // The comparator rejects validly reconstructed maps with wrong index,
        // RIP, shared-exit attribution, commit boundary, or replay safety.
        let entry_rip = 0x8100;
        let store_bytes = [0x48, 0x89, 0x03];
        let block = compile_ir_memory_store_block_v2(
            CodeView::new(entry_rip, &store_bytes).unwrap(),
            entry_rip,
        )
        .unwrap();
        let original = block.pc_map();
        let rebuild = |ranges, boundaries| {
            crate::statepoints::PcMap::new(
                original.code_len() as usize,
                original.instruction_count() as usize,
                ranges,
                boundaries,
            )
            .unwrap()
        };

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .find(|range| range.phase == StatePhase::HelperCall)
            .unwrap()
            .instruction_index = None;
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .find(|range| range.phase == StatePhase::HelperCall)
            .unwrap()
            .guest_rip = GuestRipSource::Fixed(entry_rip + 3);
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .find(|range| range.phase == StatePhase::PendingEffects)
            .unwrap()
            .instruction_index = None;
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .filter(|range| range.phase == StatePhase::Service)
            .last()
            .unwrap()
            .guest_rip = GuestRipSource::Fixed(entry_rip);
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut boundaries = original.boundaries().to_vec();
        boundaries[0].committed_instructions = 0;
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(original.ranges().to_vec(), boundaries),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .find(|range| range.phase == StatePhase::AfterHelper)
            .unwrap()
            .snapshot = SnapshotAdmissibility::ReplayCurrentInstruction;
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .find(|range| range.phase == StatePhase::CommitInProgress)
            .unwrap()
            .snapshot = SnapshotAdmissibility::ReplayCurrentInstruction;
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));

        let mut ranges = original.ranges().to_vec();
        ranges
            .iter_mut()
            .find(|range| {
                range.phase == StatePhase::Exit
                    && range.guest_rip == GuestRipSource::Fixed(entry_rip)
            })
            .unwrap()
            .snapshot = SnapshotAdmissibility::CanonicalBoundary;
        assert!(!matches_single_site_pc_map_contract(
            &rebuild(ranges, original.boundaries().to_vec()),
            entry_rip,
            MemorySiteOperationV2::Store,
        ));
    }

    #[test]
    fn ir_checked_load_uses_distinct_v2_artifact_and_pc_map() {
        let code = CodeView::new(0x1000, &[0x48, 0x8b, 0x03]).unwrap();
        let ir_block = compile_ir_memory_load_block_v2(code, 0x1000).unwrap();
        let direct = compile_memory_load_block_v2(code, 0x1000).unwrap();
        assert_eq!(
            ir_block.compile_flavor(),
            crate::translation::CompileFlavor::S3_26aIrMemoryLoadV2
        );
        assert_eq!(ir_block.site(), direct.site());
        assert_eq!(ir_block.source_bytes(), &[0x48, 0x8b, 0x03]);
        assert_eq!(ir_block.pc_map().instruction_count(), 1);
        assert!(
            ir_block
                .pc_map()
                .ranges()
                .iter()
                .any(|range| range.phase == crate::statepoints::StatePhase::HelperCall)
        );
        assert_eq!(ir_block.pc_map().boundaries().len(), 1);
        assert_ne!(ir_block.code().words(), direct.code().words());
        assert_eq!(
            direct.pc_map().code_len() as usize,
            direct.code().words().len() * 4
        );
        assert_eq!(direct.pc_map().instruction_count(), 1);
        assert_eq!(direct.pc_map().boundaries().len(), 1);
        assert_eq!(
            direct.pc_map().boundaries()[0].rip,
            crate::statepoints::GuestRipBoundary::Fixed(direct.site().next_rip())
        );
        let ranges = direct.pc_map().ranges();
        assert!(!ranges.is_empty());
        assert_eq!(ranges[0].range.start(), 0);
        assert_eq!(
            ranges.last().unwrap().range.end(),
            direct.pc_map().code_len()
        );
        for (index, range) in ranges.iter().enumerate() {
            assert_eq!(range.range.start() % 4, 0);
            assert_eq!(range.range.end() % 4, 0);
            if index > 0 {
                assert_eq!(ranges[index - 1].range.end(), range.range.start());
            }
        }
        assert!(ranges.iter().any(|range| range.snapshot
            == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction));
        assert!(ranges.iter().any(|range| range.snapshot
            == crate::statepoints::SnapshotAdmissibility::InadmissibleDuringCommit));
        let exit_ranges = ranges
            .iter()
            .filter(|range| range.phase == crate::statepoints::StatePhase::Exit)
            .collect::<Vec<_>>();
        assert_eq!(
            exit_ranges.len(),
            5,
            "each out-of-line exit stub has its own range"
        );
        assert!(
            exit_ranges.iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x1003)
            }),
            "STOP-after-commit is attributed to the fixed next RIP"
        );
        assert!(
            ranges.iter().any(|range| {
                range.phase == crate::statepoints::StatePhase::Committed
                    && range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x1003)
            }),
            "normal completion is attributed to the fixed next RIP"
        );
        assert!(
            exit_ranges.iter().any(|range| {
                range.guest_rip == crate::statepoints::GuestRipSource::Fixed(0x1000)
                    && range.snapshot
                        == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction
            }),
            "load helper failure is attributed to the fixed guest RIP"
        );

        let store = compile_memory_store_block_v2(
            CodeView::new(0x1000, &[0x48, 0x89, 0x03]).unwrap(),
            0x1000,
        )
        .unwrap();
        assert_eq!(
            store.pc_map().code_len() as usize,
            store.code().words().len() * 4
        );
        let helper = store
            .pc_map()
            .ranges()
            .iter()
            .find(|range| range.phase == crate::statepoints::StatePhase::HelperCall)
            .unwrap();
        assert_eq!(
            helper.snapshot,
            crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect
        );
        assert!(!store.pc_map().ranges().iter().any(|range| range.snapshot
            == crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction));
    }

    #[test]
    fn ir_checked_rip_relative_load_store_keep_decoded_next_rip_and_displacement() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for displacement in [i32::MIN, -1, 0, 1, i32::MAX] {
                let mut source = vec![0x48, opcode, 0x05];
                source.extend_from_slice(&displacement.to_le_bytes());
                let mut code_bytes = vec![0x90, 0x90];
                code_bytes.extend_from_slice(&source);
                let code = CodeView::new(0x5100, &code_bytes).unwrap();
                let entry = 0x5102;
                if operation == MemorySiteOperationV2::Load {
                    let (s, _) = checked_load_metadata_v2(code, entry)
                        .unwrap_or_else(|e| panic!("metadata {displacement}: {e:?}"));
                    assert_eq!(s.operand().displacement, i64::from(displacement));
                }
                let (site, got_source, pc_map) = match operation {
                    MemorySiteOperationV2::Load => {
                        let artifact = compile_ir_memory_load_block_v2(code, entry)
                            .unwrap_or_else(|e| panic!("{operation:?} disp={displacement}: {e:?}"));
                        (
                            artifact.site().clone(),
                            artifact.source_bytes().to_vec(),
                            artifact.pc_map().clone(),
                        )
                    }
                    MemorySiteOperationV2::Store => {
                        let artifact = compile_ir_memory_store_block_v2(code, entry).unwrap();
                        (
                            artifact.site().clone(),
                            artifact.source_bytes().to_vec(),
                            artifact.pc_map().clone(),
                        )
                    }
                };
                assert_eq!(got_source, source);
                assert_eq!(site.guest_rip(), entry);
                assert_eq!(site.next_rip(), entry + 7);
                assert_eq!(site.operand().base, AddressBase::NextRip);
                assert_eq!(site.operand().displacement, i64::from(displacement));
                assert_eq!(pc_map.instruction_count(), 1);
            }
        }
    }

    #[test]
    fn ir_checked_direct_disp8_disp32_sites_match_decoded_metadata() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for register in 0..4_u8 {
                let guest_register = [Register::Rax, Register::Rcx, Register::Rdx, Register::Rbx]
                    [usize::from(register)];
                for (mod_bits, displacements) in [
                    (1_u8, vec![i32::from(i8::MIN), -1, 0, i32::from(i8::MAX)]),
                    (2_u8, vec![i32::MIN, -1, 0, 1, i32::MAX]),
                ] {
                    for displacement in displacements {
                        let mut source =
                            vec![0x48, opcode, (mod_bits << 6) | (register << 3) | register];
                        if mod_bits == 1 {
                            source.push(displacement as i8 as u8);
                        } else {
                            source.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let code = CodeView::new(0x5400, &source).unwrap();
                        let (site, got_source) = match operation {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x5400).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x5400).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                (ir.site().clone(), ir.source_bytes().to_vec())
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x5400).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x5400).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                (ir.site().clone(), ir.source_bytes().to_vec())
                            }
                        };
                        assert_eq!(got_source, source);
                        assert_eq!(site.guest_rip(), 0x5400);
                        assert_eq!(site.next_rip(), 0x5400 + source.len() as u64);
                        assert_eq!(site.operand().base, AddressBase::Register(guest_register));
                        assert_eq!(site.operand().displacement, i64::from(displacement));
                        assert_eq!(site.operation(), operation);
                    }
                }
            }
        }
    }

    #[test]
    fn ir_checked_rbp_base_disp32_is_distinct_from_rip_relative_disp32() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for register in 0..4_u8 {
                for displacement in [i32::MIN, -129, -1, 0, 127, 128, i32::MAX] {
                    let mut source = vec![0x48, opcode, 0x80 | (register << 3) | 5];
                    source.extend_from_slice(&displacement.to_le_bytes());
                    let code = CodeView::new(0x5400, &source).unwrap();
                    let (site, copied) = match operation {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x5400).unwrap();
                            let direct = compile_memory_load_block_v2(code, 0x5400).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x5400).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x5400).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                    };
                    assert_eq!(copied, source);
                    assert_eq!(site.next_rip(), 0x5407);
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rbp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, i64::from(displacement));
                    assert!(site.has_supported_profile());
                    assert!(site.matches_source_bytes(&source, operation));

                    let mut rip_source = vec![0x48, opcode, (register << 3) | 5];
                    rip_source.extend_from_slice(&displacement.to_le_bytes());
                    let rip_code = CodeView::new(0x5400, &rip_source).unwrap();
                    let rip_site = match operation {
                        MemorySiteOperationV2::Load => {
                            compile_ir_memory_load_block_v2(rip_code, 0x5400)
                                .unwrap()
                                .site()
                                .clone()
                        }
                        MemorySiteOperationV2::Store => {
                            compile_ir_memory_store_block_v2(rip_code, 0x5400)
                                .unwrap()
                                .site()
                                .clone()
                        }
                    };
                    assert_eq!(rip_site.operand().base, AddressBase::NextRip);
                    assert_ne!(site.operand().base, rip_site.operand().base);
                    assert!(rip_site.matches_source_bytes(&rip_source, operation));
                }
            }
        }
    }

    #[test]
    fn rsp_is_admitted_only_as_qword_memory_data_for_exact_direct_forms() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in 0..4_u8 {
                let source = [0x48, opcode, 0x20 | base]; // data=RSP, [base]
                let code = CodeView::new(0x5800, &source).unwrap();
                let (site, copied) = match operation {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x5800).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x5800).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x5800).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x5800).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                };
                assert_eq!(copied, source);
                assert_eq!(site.next_rip(), 0x5803);
                assert_eq!(
                    site.operand().base,
                    AddressBase::Register(Register::from_encoding(base).unwrap())
                );
                assert_eq!(site.operand().index, None);
                assert_eq!(site.operand().displacement, 0);
                assert_eq!(site.operation(), operation);
                assert_eq!(
                    site.destination(),
                    (operation == MemorySiteOperationV2::Load).then_some(4)
                );
                assert_eq!(
                    site.source(),
                    (operation == MemorySiteOperationV2::Store).then_some(4)
                );
                assert!(site.has_supported_profile());
                assert!(site.matches_source_bytes(&source, operation));
            }
        }

        for invalid in [
            [0x48, 0x8b, 0x24], // RSP data with RSP base
            [0x48, 0x8b, 0x64], // RSP data with SIB
            [0x48, 0x89, 0x65], // RSP source with RBP disp8 form
        ] {
            let code = CodeView::new(0x5800, &invalid).unwrap();
            assert!(compile_ir_memory_load_block_v2(code, 0x5800).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5800).is_err());
        }
    }

    #[test]
    fn rsi_rdi_rsp_sib_qword_mov_has_exact_twelve_form_profile() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in [6_u8, 7] {
                for (mode, displacement) in
                    [(0_u8, vec![]), (1, vec![0x80]), (2, vec![0, 0, 0, 0x80])]
                {
                    let modrm = (mode << 6) | (data_register << 3) | 4;
                    let mut source = vec![0x48, opcode, modrm, 0x24];
                    source.extend_from_slice(&displacement);
                    let code = CodeView::new(0x6800, &source).unwrap();
                    let (site, copied) = match operation {
                        MemorySiteOperationV2::Load => {
                            let direct = compile_memory_load_block_v2(code, 0x6800).unwrap();
                            let ir = compile_ir_memory_load_block_v2(code, 0x6800).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                        MemorySiteOperationV2::Store => {
                            let direct = compile_memory_store_block_v2(code, 0x6800).unwrap();
                            let ir = compile_ir_memory_store_block_v2(code, 0x6800).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                    };
                    assert_eq!(copied, source);
                    assert_eq!(
                        site.source(),
                        (operation == MemorySiteOperationV2::Store).then_some(data_register)
                    );
                    assert_eq!(
                        site.destination(),
                        (operation == MemorySiteOperationV2::Load).then_some(data_register)
                    );
                    assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rsp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(
                        site.operand().displacement,
                        if mode == 0 {
                            0
                        } else if mode == 1 {
                            -128
                        } else {
                            i32::MIN as i64
                        }
                    );
                    assert_eq!(site.next_rip(), 0x6800 + source.len() as u64);
                    assert!(site.has_supported_profile());
                    assert!(site.matches_source_bytes(&source, operation));
                    if mode != 0 {
                        let mut forged = source.clone();
                        *forged.last_mut().unwrap() ^= 1;
                        assert!(
                            !site.matches_source_bytes(&forged, operation),
                            "changed displacement byte reused descriptor {source:02x?}"
                        );
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for invalid in [
                vec![0x48, opcode, 0x34, 0x20],             // index register RAX
                vec![0x48, opcode, 0x34, 0x2c],             // indexed RSP
                vec![0x48, opcode, 0x34, 0x64, 0],          // noncanonical SIB field
                vec![0x48, opcode, 0x34, 0x24, 0, 0, 0, 0], // mode/length mismatch
                vec![0x67, 0x48, opcode, 0x34, 0x24],       // address-size override
                vec![0x49, opcode, 0x34, 0x24],             // extended REX
            ] {
                let code = CodeView::new(0x6800, &invalid).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x6800).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x6800).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "unsupported neighbor admitted {invalid:02x?}"
                );
            }
            for complete in [
                vec![0x48, opcode, 0x34, 0x24],
                vec![0x48, opcode, 0x74, 0x24, 1],
                vec![0x48, opcode, 0xb4, 0x24, 1, 0, 0, 0],
            ] {
                for length in 1..complete.len() {
                    let code = CodeView::new(0x6800, &complete[..length]).unwrap();
                    let result = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(code, 0x6800).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(code, 0x6800).map(|_| ())
                    };
                    assert!(
                        result.is_err(),
                        "partial form admitted {length}: {complete:02x?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rbp_is_admitted_only_as_qword_memory_data_for_exact_direct_forms() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in 0..4_u8 {
                let source = [0x48, opcode, 0x28 | base]; // data=RBP, [base]
                let code = CodeView::new(0x5900, &source).unwrap();
                let (site, copied) = match operation {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x5900).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x5900).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x5900).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x5900).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                };
                assert_eq!(copied, source);
                assert_eq!(site.next_rip(), 0x5903);
                assert_eq!(
                    site.operand().base,
                    AddressBase::Register(Register::from_encoding(base).unwrap())
                );
                assert_eq!(site.operand().index, None);
                assert_eq!(site.operand().displacement, 0);
                assert_eq!(site.operation(), operation);
                assert_eq!(
                    site.destination(),
                    (operation == MemorySiteOperationV2::Load).then_some(5)
                );
                assert_eq!(
                    site.source(),
                    (operation == MemorySiteOperationV2::Store).then_some(5)
                );
                assert!(site.has_supported_profile());
                assert!(site.matches_source_bytes(&source, operation));
            }
        }

        for invalid in [
            vec![0x48, 0x8b, 0x2c],             // SIB
            vec![0x48, 0x8b, 0x68],             // displacement
            vec![0x67, 0x48, 0x8b, 0x3c, 0x20], // SIB
        ] {
            let code = CodeView::new(0x5900, &invalid).unwrap();
            assert!(compile_ir_memory_load_block_v2(code, 0x5900).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5900).is_err());
        }
        let rsp_data_pair = [0x48, 0x8b, 0x28, 0x48, 0x89, 0x02];
        assert!(
            compile_ir_memory_two_site_block_v2(
                CodeView::new(0x5900, &rsp_data_pair).unwrap(),
                0x5900
            )
            .is_err()
        );
    }

    #[test]
    fn rsi_rdi_store_sources_are_limited_to_the_eight_direct_low_base_encodings() {
        for source_register in [6_u8, 7] {
            for base in 0..4_u8 {
                let bytes = [0x48, 0x89, (source_register << 3) | base];
                let code = CodeView::new(0x5c00, &bytes).unwrap();
                let direct = compile_memory_store_block_v2(code, 0x5c00).unwrap();
                let ir = compile_ir_memory_store_block_v2(code, 0x5c00).unwrap();
                assert_eq!(direct.site(), ir.site());
                assert_eq!(direct.source_bytes(), &bytes);
                assert_eq!(ir.source_bytes(), &bytes);
                assert_eq!(direct.site().source(), Some(source_register));
                assert!(direct.site().has_supported_profile());
                assert!(
                    direct
                        .site()
                        .matches_source_bytes(&bytes, MemorySiteOperationV2::Store)
                );
            }
        }

        for source in [
            // Address32 RSI/RDI data has a separate exact profile.
            vec![0x48, 0x89, 0x34, 0x20], // SIB
            vec![0x89, 0x30],             // non-qword width
            vec![0x49, 0x89, 0x30],       // REX.B
            vec![0x4c, 0x89, 0x30],       // REX.R
            vec![0x4c, 0x89, 0x00],       // R8 source
            vec![0x66, 0x48, 0x89, 0x30], // word width
        ] {
            let code = CodeView::new(0x5c00, &source).unwrap();
            assert!(compile_memory_store_block_v2(code, 0x5c00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5c00).is_err());
            assert!(compile_memory_load_block_v2(code, 0x5c00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5c00).is_err());
        }
    }

    #[test]
    fn rsi_rdi_load_destinations_are_limited_to_eight_direct_low_base_encodings() {
        for destination in [6_u8, 7] {
            for base in 0..4_u8 {
                let bytes = [0x48, 0x8b, (destination << 3) | base];
                let code = CodeView::new(0x5d00, &bytes).unwrap();
                let direct = compile_memory_load_block_v2(code, 0x5d00).unwrap();
                let ir = compile_ir_memory_load_block_v2(code, 0x5d00).unwrap();
                assert_eq!(direct.site(), ir.site());
                assert_eq!(direct.source_bytes(), &bytes);
                assert_eq!(ir.source_bytes(), &bytes);
                assert_eq!(direct.site().destination(), Some(destination));
                assert_eq!(direct.site().next_rip(), 0x5d03);
                assert!(direct.site().has_supported_profile());
                assert!(
                    direct
                        .site()
                        .matches_source_bytes(&bytes, MemorySiteOperationV2::Load)
                );
            }
        }

        for bytes in [
            vec![0x48, 0x8b, 0x76, 0x00], // displacement
            vec![0x48, 0x8b, 0x34, 0x20], // SIB
            vec![0x40, 0x48, 0x8b, 0x30], // extra REX prefix
            vec![0x49, 0x8b, 0x30],       // REX.B
            vec![0x4c, 0x8b, 0x30],       // REX.R
            vec![0x4c, 0x8b, 0x00],       // R8 destination
            vec![0x8b, 0x30],             // non-qword width
            vec![0x66, 0x48, 0x8b, 0x30], // word width
        ] {
            let code = CodeView::new(0x5d00, &bytes).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5d00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5d00).is_err());
        }
    }

    #[test]
    fn rsi_rdi_data_direct_displacement_profile_is_exactly_40_encodings() {
        let displacements = [
            (1_u8, i8::MIN as i64),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i64),
            (2, i32::MIN as i64),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX as i64),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in [6_u8, 7] {
                for base in [0_u8, 1, 2, 3, 5] {
                    for (modrm_mode, displacement) in displacements {
                        let mut bytes = vec![0x48, opcode, (modrm_mode << 6) | (data << 3) | base];
                        if modrm_mode == 1 {
                            bytes.push(displacement as i8 as u8);
                        } else {
                            bytes.extend_from_slice(&(displacement as i32).to_le_bytes());
                        }
                        let code = CodeView::new(0x5e00, &bytes).unwrap();
                        let decoded = long_decode::decode(
                            &mut CodeMemory::full_code(code),
                            0x5e00,
                            AccessContext::default(),
                        )
                        .unwrap();
                        assert!(
                            match operation {
                                MemorySiteOperationV2::Load =>
                                    crate::block::exact_direct_qword_load_rsi_rdi_bytes(
                                        code,
                                        0x5e00,
                                        decoded.length
                                    ),
                                MemorySiteOperationV2::Store =>
                                    crate::block::exact_direct_qword_store_rsi_rdi_bytes(
                                        code,
                                        0x5e00,
                                        decoded.length
                                    ),
                            },
                            "exact-byte predicate rejected {bytes:02x?}"
                        );
                        assert!(
                            match operation {
                                MemorySiteOperationV2::Load =>
                                    crate::block::is_checked_qword_load_form(decoded),
                                MemorySiteOperationV2::Store =>
                                    crate::block::is_checked_qword_store_form(decoded),
                            },
                            "checked-form predicate rejected {bytes:02x?}"
                        );
                        match operation {
                            MemorySiteOperationV2::Load => {
                                let direct = compile_memory_load_block_v2(code, 0x5e00).unwrap();
                                let ir = compile_ir_memory_load_block_v2(code, 0x5e00).unwrap();
                                assert_eq!(direct.site(), ir.site());
                                assert_eq!(direct.source_bytes(), bytes);
                                assert_eq!(ir.source_bytes(), bytes);
                                assert_eq!(direct.site().next_rip(), 0x5e00 + bytes.len() as u64);
                                assert_eq!(direct.site().operand().displacement, displacement);
                                assert!(direct.site().has_supported_profile());
                                assert!(direct.site().matches_source_bytes(&bytes, operation));
                                if modrm_mode == 2 {
                                    let mut changed_final_disp = bytes.clone();
                                    *changed_final_disp.last_mut().unwrap() ^= 1;
                                    assert!(
                                        !direct
                                            .site()
                                            .matches_source_bytes(&changed_final_disp, operation)
                                    );
                                }
                                assert!(crate::block::is_checked_qword_load_form(decoded));
                            }
                            MemorySiteOperationV2::Store => {
                                let direct = compile_memory_store_block_v2(code, 0x5e00).unwrap();
                                let ir = compile_ir_memory_store_block_v2(code, 0x5e00).unwrap();
                                assert_eq!(direct.site(), ir.site());
                                assert_eq!(direct.source_bytes(), bytes);
                                assert_eq!(ir.source_bytes(), bytes);
                                assert_eq!(direct.site().next_rip(), 0x5e00 + bytes.len() as u64);
                                assert_eq!(direct.site().operand().displacement, displacement);
                                assert!(direct.site().has_supported_profile());
                                assert!(direct.site().matches_source_bytes(&bytes, operation));
                                if modrm_mode == 2 {
                                    let mut changed_final_disp = bytes.clone();
                                    *changed_final_disp.last_mut().unwrap() ^= 1;
                                    assert!(
                                        !direct
                                            .site()
                                            .matches_source_bytes(&changed_final_disp, operation)
                                    );
                                }
                                assert!(crate::block::is_checked_qword_store_form(decoded));
                            }
                        }
                    }
                }
            }
        }

        for bytes in [
            vec![0x48, 0x8b, 0x76, 0],          // RSI data and base
            vec![0x48, 0x89, 0xbf, 0, 0, 0, 0], // RDI data, RDI base
            vec![0x48, 0x8b, 0x74, 0x20, 0],    // SIB
            vec![0x49, 0x8b, 0x70, 0],          // REX.B
            vec![0x4c, 0x8b, 0x30, 0],          // REX.R / R8 destination
            vec![0x67, 0x48, 0x8b, 0x75, 0],    // address-size override
            vec![0x48, 0x8b, 0x34, 0x20],       // SIB base RAX, index RSP
            vec![0x66, 0x48, 0x8b, 0x30],       // operand override
        ] {
            let code = CodeView::new(0x5e00, &bytes).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5e00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5e00).is_err());
            assert!(compile_memory_store_block_v2(code, 0x5e00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5e00).is_err());
        }

        for bytes in [
            &[0x48, 0x8b, 0x30][..],
            &[0x48, 0x8b, 0x70, 0][..],
            &[0x48, 0x8b, 0xb0, 0, 0, 0, 0][..],
        ] {
            for length in 1..bytes.len() {
                let code = CodeView::new(0x5e00, &bytes[..length]).unwrap();
                assert!(compile_memory_load_block_v2(code, 0x5e00).is_err());
                assert!(compile_ir_memory_load_block_v2(code, 0x5e00).is_err());
                assert!(compile_memory_store_block_v2(code, 0x5e00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5e00).is_err());
            }
        }
    }

    #[test]
    fn rsi_rdi_are_only_admitted_as_exact_qword_direct_bases() {
        let displacement_forms = [
            (0_u8, 0_i64),
            (1, i8::MIN as i64),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i64),
            (2, i32::MIN as i64),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX as i64),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..6_u8 {
                for base in [6_u8, 7] {
                    for (mode, displacement) in displacement_forms {
                        let mut source = vec![0x48, opcode, (mode << 6) | (data << 3) | base];
                        match mode {
                            0 => {}
                            1 => source.push(displacement as i8 as u8),
                            2 => source.extend_from_slice(&(displacement as i32).to_le_bytes()),
                            _ => unreachable!(),
                        }
                        let code = CodeView::new(0x5a00, &source).unwrap();
                        let (site, copied) = match operation {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x5a00).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x5a00).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                (ir.site().clone(), ir.source_bytes().to_vec())
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x5a00).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x5a00).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                (ir.site().clone(), ir.source_bytes().to_vec())
                            }
                        };
                        assert_eq!(copied, source);
                        assert_eq!(site.next_rip(), 0x5a00 + source.len() as u64);
                        assert_eq!(
                            site.operand().base,
                            AddressBase::Register(Register::from_encoding(base).unwrap())
                        );
                        assert_eq!(site.operand().index, None);
                        assert_eq!(site.operand().scale, 1);
                        assert_eq!(site.operand().displacement, displacement);
                        assert_eq!(site.operation(), operation);
                        assert_eq!(
                            site.destination(),
                            (operation == MemorySiteOperationV2::Load).then_some(data)
                        );
                        assert_eq!(
                            site.source(),
                            (operation == MemorySiteOperationV2::Store).then_some(data)
                        );
                        assert!(site.has_supported_profile());
                        assert!(site.matches_source_bytes(&source, operation));
                    }
                }
            }
        }

        for invalid in [
            vec![0x48, 0x8b, 0x76, 0x00],       // RSI data with RSI + disp8
            vec![0x48, 0x89, 0xbf, 0, 0, 0, 0], // RDI data with RDI + disp32
            vec![0x48, 0x8b, 0x04, 0x66],       // SIB with RSI base and no-index scale alias
            vec![0x48, 0x8b, 0x36],             // RSI as data
            vec![0x48, 0x8b, 0x3e],             // RDI as data
            vec![0x48, 0x8b, 0x46],             // truncated disp8
            vec![0x48, 0x8b, 0x86, 0, 0, 0],    // truncated disp32
            vec![0x48],                         // partial opcode
            vec![0x48, 0x8b],                   // partial ModRM
        ] {
            let code = CodeView::new(0x5a00, &invalid).unwrap();
            assert!(compile_ir_memory_load_block_v2(code, 0x5a00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5a00).is_err());
        }
    }

    #[test]
    fn address32_rsi_rdi_data_has_exact_48_direct_low_base_forms() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in [6_u8, 7] {
                for base in 0..4_u8 {
                    for (mode, displacement) in [
                        (0_u8, 0_i64),
                        (1, i8::MIN as i64),
                        (1, -1),
                        (1, 0),
                        (1, i8::MAX as i64),
                        (2, i32::MIN as i64),
                        (2, -1),
                        (2, 0),
                        (2, 1),
                        (2, i32::MAX as i64),
                    ] {
                        let mut bytes = vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | base];
                        if mode == 1 {
                            bytes.push(displacement as i8 as u8);
                        } else if mode == 2 {
                            bytes.extend_from_slice(&(displacement as i32).to_le_bytes());
                        }
                        let code = CodeView::new(0x5f00, &bytes).unwrap();
                        let (direct, ir, direct_bytes, ir_bytes) = match operation {
                            MemorySiteOperationV2::Load => {
                                let direct = compile_memory_load_block_v2(code, 0x5f00).unwrap();
                                assert!(
                                    direct.site().has_supported_profile(),
                                    "direct profile rejected {bytes:02x?}"
                                );
                                assert!(
                                    direct.site().matches_source_bytes(&bytes, operation),
                                    "source rejected {bytes:02x?}"
                                );
                                let ir = compile_ir_memory_load_block_v2(code, 0x5f00).unwrap();
                                (
                                    direct.site().clone(),
                                    ir.site().clone(),
                                    direct.source_bytes().to_vec(),
                                    ir.source_bytes().to_vec(),
                                )
                            }
                            MemorySiteOperationV2::Store => {
                                let direct = compile_memory_store_block_v2(code, 0x5f00).unwrap();
                                let ir = compile_ir_memory_store_block_v2(code, 0x5f00).unwrap();
                                (
                                    direct.site().clone(),
                                    ir.site().clone(),
                                    direct.source_bytes().to_vec(),
                                    ir.source_bytes().to_vec(),
                                )
                            }
                        };
                        assert_eq!(direct, ir);
                        assert_eq!(direct_bytes, bytes);
                        assert_eq!(ir_bytes, bytes);
                        assert_eq!(direct.destination(), (opcode == 0x8b).then_some(data));
                        assert_eq!(direct.source(), (opcode == 0x89).then_some(data));
                        assert_eq!(direct.operand().address_size, od_isa::OperandSize::Dword);
                        assert_eq!(direct.operand().size, od_isa::OperandSize::Qword);
                        assert_eq!(
                            direct.operand().base,
                            AddressBase::Register(Register::from_encoding(base).unwrap())
                        );
                        assert_eq!(direct.operand().index, None);
                        assert_eq!(direct.operand().displacement, displacement);
                        assert!(direct.has_supported_profile());
                        assert!(direct.matches_source_bytes(&bytes, operation));
                        let mut forged = direct;
                        forged.operand.address_size = od_isa::OperandSize::Qword;
                        assert!(!forged.matches_source_bytes(&bytes, operation));
                        let mut forged = direct;
                        forged.operand.base = AddressBase::Register(Register::Rsi);
                        assert!(!forged.matches_source_bytes(&bytes, operation));
                        if mode == 2 {
                            let mut changed = bytes.clone();
                            *changed.last_mut().unwrap() ^= 1;
                            assert!(!direct.matches_source_bytes(&changed, operation));
                        }
                    }
                }
            }
        }
        for complete in [
            vec![0x67, 0x48, 0x8b, 0x30],
            vec![0x67, 0x48, 0x89, 0x70, 0x80],
            vec![0x67, 0x48, 0x8b, 0xb0, 0, 0, 0, 0x80],
        ] {
            for window in 1..complete.len() {
                let code = CodeView::new(0x5f00, &complete[..window]).unwrap();
                assert!(compile_memory_load_block_v2(code, 0x5f00).is_err());
                assert!(compile_ir_memory_load_block_v2(code, 0x5f00).is_err());
                assert!(compile_memory_store_block_v2(code, 0x5f00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5f00).is_err());
            }
        }
        for invalid in [
            vec![0x67, 0x48, 0x8b, 0x34, 0x20],
            vec![0x67, 0x49, 0x8b, 0x30],
            vec![0x67, 0x4c, 0x8b, 0x30],
            vec![0x67, 0x48, 0x67, 0x8b, 0x30],
        ] {
            let code = CodeView::new(0x5f00, &invalid).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5f00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5f00).is_err());
            assert!(compile_memory_store_block_v2(code, 0x5f00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5f00).is_err());
        }
    }

    #[test]
    fn address32_rsi_rdi_direct_qword_bases_have_exact_displacement_profiles() {
        let displacement_forms = [
            (0_u8, 0_i64),
            (1, i8::MIN as i64),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i64),
            (2, i32::MIN as i64),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX as i64),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..6_u8 {
                for base in [6_u8, 7] {
                    for (mode, displacement) in displacement_forms {
                        let mut source = vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | base];
                        match mode {
                            0 => {}
                            1 => source.push(displacement as i8 as u8),
                            2 => source.extend_from_slice(&(displacement as i32).to_le_bytes()),
                            _ => unreachable!(),
                        }
                        let code = CodeView::new(0x5b00, &source).unwrap();
                        let (site, copied) = match operation {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x5b00).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x5b00).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                (ir.site().clone(), ir.source_bytes().to_vec())
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x5b00).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x5b00).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                (ir.site().clone(), ir.source_bytes().to_vec())
                            }
                        };
                        let expected_base = if base == 6 {
                            Register::Rsi
                        } else {
                            Register::Rdi
                        };
                        assert_eq!(copied, source);
                        assert_eq!(site.next_rip(), 0x5b00 + source.len() as u64);
                        assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                        assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                        assert_eq!(site.operand().base, AddressBase::Register(expected_base));
                        assert_eq!(site.operand().index, None);
                        assert_eq!(site.operand().scale, 1);
                        assert_eq!(site.operand().displacement, displacement);
                        assert_eq!(site.operation(), operation);
                        assert_eq!(
                            site.destination(),
                            (operation == MemorySiteOperationV2::Load).then_some(data)
                        );
                        assert_eq!(
                            site.source(),
                            (operation == MemorySiteOperationV2::Store).then_some(data)
                        );
                        assert!(site.has_supported_profile());
                        assert!(site.matches_source_bytes(&source, operation));
                    }
                }
            }
        }

        for invalid in [
            vec![0x67, 0x48, 0x8b, 0x3f], // RDI data with RDI base remains excluded
            vec![0x67, 0x48, 0x8b, 0x76], // partial disp8
            vec![0x67, 0x48, 0x8b, 0x86, 0, 0, 0], // partial disp32
            vec![0x67, 0x48, 0x67, 0x8b, 0x06], // extra/reordered prefix
        ] {
            let code = CodeView::new(0x5b00, &invalid).unwrap();
            assert!(compile_ir_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5b00).is_err());
            assert!(compile_memory_store_block_v2(code, 0x5b00).is_err());
        }
    }

    #[test]
    fn low_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in 0..4_u8 {
                for index in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            let source = [
                                0x48,
                                opcode,
                                (data << 3) | 4,
                                (scale_shift << 6) | (index << 3) | base,
                            ];
                            let code = CodeView::new(0x5b00, &source).unwrap();
                            let mut decoder_memory = CodeMemory::full_code(code);
                            let decoded = long_decode::decode(
                                &mut decoder_memory,
                                0x5b00,
                                AccessContext::default(),
                            )
                            .unwrap();
                            if operation == MemorySiteOperationV2::Load {
                                assert!(crate::block::is_checked_qword_load_form(decoded));
                                crate::block_compile::compile_checked_load_block_v2(code, 0x5b00)
                                    .unwrap();
                            } else {
                                assert!(crate::block::is_checked_qword_store_form(decoded));
                            }
                            let (site, copied) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x5b00).unwrap();
                                    assert!(direct.site().has_supported_profile());
                                    assert!(direct.site().matches_source_bytes(&source, operation));
                                    let ir = compile_ir_memory_load_block_v2(code, 0x5b00).unwrap();
                                    (ir.site().clone(), ir.source_bytes().to_vec())
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x5b00).unwrap();
                                    assert!(direct.site().has_supported_profile());
                                    assert!(direct.site().matches_source_bytes(&source, operation));
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x5b00).unwrap();
                                    (ir.site().clone(), ir.source_bytes().to_vec())
                                }
                            };
                            assert_eq!(copied, source);
                            assert_eq!(site.guest_rip(), 0x5b00);
                            assert_eq!(site.next_rip(), 0x5b04);
                            assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(site.operand().index, Register::from_encoding(index));
                            assert_eq!(site.operand().scale, 1 << scale_shift);
                            assert_eq!(site.operand().displacement, 0);
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&source, operation));
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x48, 0x8b, 0x04, 0x60],       // no-index SIB with non-unit scale
            vec![0x48, 0x8b, 0x04, 0x66],       // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x67],       // RDI base with no-index scale alias
            vec![0x48, 0x8b, 0x74, 0x25, 0],    // RSI data is outside this profile
            vec![0x67, 0x48, 0x8b, 0x34, 0x34], // RSI data is outside this profile
            vec![0x48, 0x8b, 0x04],             // truncated SIB
        ] {
            let code = CodeView::new(0x5b00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
        }
        let full = [0x48, 0x8b, 0x04, 0x30];
        for length in 1..full.len() {
            let code = CodeView::new(0x5b00, &full[..length]).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_memory_store_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5b00).is_err());
        }
    }

    #[test]
    fn low_base_rsi_rdi_index_sib_displacements_have_exact_qword_profile() {
        let displacement_cases = [
            (1_u8, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in 0..4_u8 {
                for index in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            for (mode, displacement) in displacement_cases {
                                let mut source = vec![
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data << 3) | 4,
                                    (scale_shift << 6) | (index << 3) | base,
                                ];
                                match mode {
                                    1 => source.push(displacement as i8 as u8),
                                    2 => source.extend_from_slice(&displacement.to_le_bytes()),
                                    _ => unreachable!(),
                                }
                                let code = CodeView::new(0x5b00, &source).unwrap();
                                let mut decoder_memory = CodeMemory::full_code(code);
                                let decoded = long_decode::decode(
                                    &mut decoder_memory,
                                    0x5b00,
                                    AccessContext::default(),
                                )
                                .unwrap();
                                let operand = match decoded.operation {
                                    od_isa::long_decode::Operation::Mov {
                                        dst: od_isa::long_decode::Operand::Register(_),
                                        src: od_isa::long_decode::Operand::Memory(operand),
                                    }
                                    | od_isa::long_decode::Operation::Mov {
                                        dst: od_isa::long_decode::Operand::Memory(operand),
                                        src: od_isa::long_decode::Operand::Register(_),
                                    } => operand,
                                    _ => panic!("expected memory MOV: {source:02x?}"),
                                };
                                assert_eq!(operand.address_size, od_isa::OperandSize::Qword);
                                assert_eq!(operand.size, od_isa::OperandSize::Qword);
                                assert_eq!(operand.displacement, i64::from(displacement));

                                let (direct_site, ir_site, copied) = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let direct = compile_memory_load_block_v2(code, 0x5b00)
                                            .unwrap_or_else(|error| {
                                                panic!("direct {source:02x?}: {error:?}")
                                            });
                                        let ir = compile_ir_memory_load_block_v2(code, 0x5b00)
                                            .unwrap_or_else(|error| {
                                                panic!("IR {source:02x?}: {error:?}")
                                            });
                                        (
                                            direct.site().clone(),
                                            ir.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let direct = compile_memory_store_block_v2(code, 0x5b00)
                                            .unwrap_or_else(|error| {
                                                panic!("direct {source:02x?}: {error:?}")
                                            });
                                        let ir = compile_ir_memory_store_block_v2(code, 0x5b00)
                                            .unwrap_or_else(|error| {
                                                panic!("IR {source:02x?}: {error:?}")
                                            });
                                        (
                                            direct.site().clone(),
                                            ir.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                };
                                assert_eq!(copied, source);
                                for site in [direct_site, ir_site] {
                                    assert!(site.has_supported_profile());
                                    assert!(site.matches_source_bytes(&source, operation));
                                    assert_eq!(site.next_rip(), 0x5b00 + source.len() as u64);
                                    assert_eq!(site.operand(), operand);
                                }
                            }
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x48, 0x8b, 0x74, 0x25, 0], // RSI data is outside this profile
            vec![0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0], // invalid no-index scale
            vec![0x67, 0x48, 0x8b, 0x74, 0x34, 0], // RSI data is outside this profile
            vec![0x48, 0x8b, 0x44, 0x30],    // partial disp8
            vec![0x48, 0x8b, 0x84, 0x30, 0, 0, 0], // partial disp32
        ] {
            let code = CodeView::new(0x5b00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
        }
        for full in [
            vec![0x48, 0x8b, 0x44, 0x30, 0xff],
            vec![0x48, 0x8b, 0x84, 0x30, 0, 0, 0, 0x80],
        ] {
            for length in 1..full.len() {
                let code = CodeView::new(0x5b00, &full[..length]).unwrap();
                assert!(compile_memory_load_block_v2(code, 0x5b00).is_err());
                assert!(compile_memory_store_block_v2(code, 0x5b00).is_err());
                assert!(compile_ir_memory_load_block_v2(code, 0x5b00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5b00).is_err());
            }
        }
    }

    #[test]
    fn address32_low_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in 0..4_u8 {
                for index in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            for (mode, displacement) in displacement_cases {
                                let mut source = vec![
                                    0x67,
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data << 3) | 4,
                                    (scale_shift << 6) | (index << 3) | base,
                                ];
                                match mode {
                                    0 => {}
                                    1 => source.push(displacement as i8 as u8),
                                    2 => source.extend_from_slice(&displacement.to_le_bytes()),
                                    _ => unreachable!(),
                                }
                                let code = CodeView::new(0x5c00, &source).unwrap();
                                let mut decoder_memory = CodeMemory::full_code(code);
                                let decoded = long_decode::decode(
                                    &mut decoder_memory,
                                    0x5c00,
                                    AccessContext::default(),
                                )
                                .unwrap();
                                let operand = match decoded.operation {
                                    od_isa::long_decode::Operation::Mov {
                                        dst: od_isa::long_decode::Operand::Register(_),
                                        src: od_isa::long_decode::Operand::Memory(operand),
                                    }
                                    | od_isa::long_decode::Operation::Mov {
                                        dst: od_isa::long_decode::Operand::Memory(operand),
                                        src: od_isa::long_decode::Operand::Register(_),
                                    } => operand,
                                    _ => panic!("expected memory MOV: {source:02x?}"),
                                };
                                assert_eq!(operand.address_size, od_isa::OperandSize::Dword);
                                assert_eq!(operand.size, od_isa::OperandSize::Qword);
                                assert_eq!(operand.displacement, i64::from(displacement));

                                let (direct_site, ir_site, copied) = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let direct = compile_memory_load_block_v2(code, 0x5c00)
                                            .unwrap_or_else(|error| {
                                                panic!("direct {source:02x?}: {error:?}")
                                            });
                                        let ir = compile_ir_memory_load_block_v2(code, 0x5c00)
                                            .unwrap_or_else(|error| {
                                                panic!("IR {source:02x?}: {error:?}")
                                            });
                                        (
                                            direct.site().clone(),
                                            ir.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let direct = compile_memory_store_block_v2(code, 0x5c00)
                                            .unwrap_or_else(|error| {
                                                panic!("direct {source:02x?}: {error:?}")
                                            });
                                        let ir = compile_ir_memory_store_block_v2(code, 0x5c00)
                                            .unwrap_or_else(|error| {
                                                panic!("IR {source:02x?}: {error:?}")
                                            });
                                        (
                                            direct.site().clone(),
                                            ir.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                };
                                assert_eq!(copied, source);
                                for site in [direct_site, ir_site] {
                                    assert!(site.has_supported_profile());
                                    assert!(site.matches_source_bytes(&source, operation));
                                    assert_eq!(site.next_rip(), 0x5c00 + source.len() as u64);
                                    assert_eq!(site.operand(), operand);
                                }
                            }
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x67, 0x48, 0x8b, 0x74, 0x36, 0], // unsupported RSI data register
            vec![0x67, 0x48, 0x8b, 0x74, 0x34, 0], // RSI data
            vec![0x67, 0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0], // no index with non-unit scale
            vec![0x48, 0x8b, 0x74, 0x25, 0],       // RSI data is outside this profile
        ] {
            let code = CodeView::new(0x5c00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
        }
        for full in [
            vec![0x67, 0x48, 0x8b, 0x44, 0x70, 0],
            vec![0x67, 0x48, 0x8b, 0x84, 0x70, 0, 0, 0, 0x80],
        ] {
            for length in 1..full.len() {
                let code = CodeView::new(0x5c00, &full[..length]).unwrap();
                assert!(compile_memory_load_block_v2(code, 0x5c00).is_err());
                assert!(compile_memory_store_block_v2(code, 0x5c00).is_err());
                assert!(compile_ir_memory_load_block_v2(code, 0x5c00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5c00).is_err());
            }
        }
    }

    #[test]
    fn rsi_rdi_indexed_sib_no_displacement_has_exact_qword_profile() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for index in 0..4_u8 {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            let source = [
                                0x48,
                                opcode,
                                (data << 3) | 4,
                                (scale_shift << 6) | (index << 3) | base,
                            ];
                            let code = CodeView::new(0x5b00, &source).unwrap();
                            let mut decoder_memory = CodeMemory::full_code(code);
                            let decoded = long_decode::decode(
                                &mut decoder_memory,
                                0x5b00,
                                AccessContext::default(),
                            )
                            .unwrap();
                            match operation {
                                MemorySiteOperationV2::Load => {
                                    assert!(crate::block::is_checked_qword_load_form(decoded));
                                }
                                MemorySiteOperationV2::Store => {
                                    assert!(crate::block::is_checked_qword_store_form(decoded));
                                }
                            }
                            if operation == MemorySiteOperationV2::Load {
                                crate::block_compile::compile_checked_load_block_v2(code, 0x5b00)
                                    .unwrap_or_else(|error| {
                                        panic!("block compile {source:02x?}: {error:?}")
                                    });
                            }
                            assert!(crate::block::exact_direct_qword_memory_bytes(
                                code, 0x5b00, 4, opcode
                            ));
                            let direct_site_ok = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct = compile_memory_load_block_v2(code, 0x5b00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    direct.site().has_supported_profile()
                                        && direct.site().matches_source_bytes(&source, operation)
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct = compile_memory_store_block_v2(code, 0x5b00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    direct.site().has_supported_profile()
                                        && direct.site().matches_source_bytes(&source, operation)
                                }
                            };
                            assert!(direct_site_ok, "direct site rejected {source:02x?}");
                            let (ir_site, copied) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x5b00)
                                        .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                                    (ir.site().clone(), ir.source_bytes().to_vec())
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x5b00).unwrap();
                                    (ir.site().clone(), ir.source_bytes().to_vec())
                                }
                            };
                            assert_eq!(copied, source);
                            assert_eq!(ir_site.guest_rip(), 0x5b00);
                            assert_eq!(ir_site.next_rip(), 0x5b04);
                            assert_eq!(ir_site.operand().address_size, od_isa::OperandSize::Qword);
                            assert_eq!(ir_site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                ir_site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(
                                ir_site.operand().index,
                                Some(Register::from_encoding(index).unwrap())
                            );
                            assert_eq!(ir_site.operand().scale, 1 << scale_shift);
                            assert_eq!(ir_site.operand().displacement, 0);
                            assert!(ir_site.has_supported_profile());
                            assert!(ir_site.matches_source_bytes(&source, operation));
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x48, 0x8b, 0x04, 0x66], // SIB scale alias with RSI base
            vec![0x48, 0x8b, 0x04, 0x67], // SIB scale alias with RDI base
            vec![0x48, 0x8b, 0x04, 0x66], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x67], // RDI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0], // non-unit scale with no index
            vec![0x48, 0x8b, 0x44, 0x66, 0], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x84, 0x66, 0, 0, 0, 0], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x04],       // missing SIB byte
        ] {
            let code = CodeView::new(0x5b00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
        }
        for length in 1..4 {
            let source = [0x48, 0x8b, 0x04, 0x0e];
            let code = CodeView::new(0x5b00, &source[..length]).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_memory_store_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5b00).is_err());
        }
    }

    #[test]
    fn rsi_rdi_base_rbp_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for scale_shift in 0..4_u8 {
                    for data in 0..6_u8 {
                        for &(mode, displacement) in &displacement_cases {
                            let sib = (scale_shift << 6) | (5 << 3) | base;
                            let mut source = vec![0x48, opcode, (mode << 6) | (data << 3) | 4, sib];
                            match mode {
                                0 => {}
                                1 => source.push(displacement as u8),
                                _ => source.extend_from_slice(&displacement.to_le_bytes()),
                            }
                            let code = CodeView::new(0x5b00, &source).unwrap();
                            let mut decoder_memory = CodeMemory::full_code(code);
                            let decoded = long_decode::decode(
                                &mut decoder_memory,
                                0x5b00,
                                AccessContext::default(),
                            )
                            .unwrap();
                            if operation == MemorySiteOperationV2::Load {
                                assert!(crate::block::is_checked_qword_load_form(decoded));
                                crate::block_compile::compile_checked_load_block_v2(code, 0x5b00)
                                    .unwrap_or_else(|error| {
                                        panic!("checked {source:02x?}: {error:?}")
                                    });
                            } else {
                                assert!(crate::block::is_checked_qword_store_form(decoded));
                            }
                            assert!(crate::block::exact_direct_qword_memory_bytes(
                                code,
                                0x5b00,
                                source.len() as u8,
                                opcode
                            ));
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x5b00).unwrap();
                                    assert!(direct.site().matches_source_bytes(&source, operation));
                                    let ir = compile_ir_memory_load_block_v2(code, 0x5b00).unwrap();
                                    assert_eq!(ir.source_bytes(), source);
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x5b00).unwrap();
                                    assert!(direct.site().matches_source_bytes(&source, operation));
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x5b00).unwrap();
                                    assert_eq!(ir.source_bytes(), source);
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.instruction_index(), 0);
                            assert_eq!(site.next_rip(), 0x5b00 + source.len() as u64);
                            assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(site.operand().index, Some(Register::Rbp));
                            assert_eq!(site.operand().scale, 1 << scale_shift);
                            assert_eq!(site.operand().displacement, i64::from(displacement));
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x48, 0x8b, 0x04, 0x66], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x67], // RDI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0], // invalid no-index scale
            vec![0x48, 0x8b, 0x44, 0x66, 0], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x84, 0x66, 0, 0, 0, 0], // RSI base with no-index scale alias
        ] {
            let code = CodeView::new(0x5b00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
        }
        let full = [0x48, 0x8b, 0x84, 0x6e, 0xff, 0, 0, 0];
        for length in 1..full.len() {
            let code = CodeView::new(0x5b00, &full[..length]).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_memory_store_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5b00).is_err());
            assert!(compile_ir_memory_store_block_v2(code, 0x5b00).is_err());
        }
    }

    #[test]
    fn rsi_rdi_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for index in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            for &(mode, displacement) in &displacement_cases {
                                let sib = (scale_shift << 6) | (index << 3) | base;
                                let mut source =
                                    vec![0x48, opcode, (mode << 6) | (data << 3) | 4, sib];
                                match mode {
                                    0 => {}
                                    1 => source.push(displacement as u8),
                                    _ => source.extend_from_slice(&displacement.to_le_bytes()),
                                }
                                let code = CodeView::new(0x5b00, &source).unwrap();
                                let mut decoder_memory = CodeMemory::full_code(code);
                                let decoded = long_decode::decode(
                                    &mut decoder_memory,
                                    0x5b00,
                                    AccessContext::default(),
                                )
                                .unwrap();
                                match operation {
                                    MemorySiteOperationV2::Load => {
                                        assert!(crate::block::is_checked_qword_load_form(decoded));
                                    }
                                    MemorySiteOperationV2::Store => {
                                        assert!(crate::block::is_checked_qword_store_form(decoded));
                                    }
                                }
                                let site = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x5b00).unwrap();
                                        assert!(
                                            direct.site().matches_source_bytes(&source, operation)
                                        );
                                        let ir =
                                            compile_ir_memory_load_block_v2(code, 0x5b00).unwrap();
                                        assert_eq!(ir.source_bytes(), source);
                                        ir.site().clone()
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x5b00).unwrap();
                                        assert!(
                                            direct.site().matches_source_bytes(&source, operation)
                                        );
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x5b00).unwrap();
                                        assert_eq!(ir.source_bytes(), source);
                                        ir.site().clone()
                                    }
                                };
                                assert!(site.has_supported_profile());
                                assert!(site.matches_source_bytes(&source, operation));
                                assert_eq!(site.instruction_index(), 0);
                                assert_eq!(site.next_rip(), 0x5b00 + source.len() as u64);
                                assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                                assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand().base,
                                    AddressBase::Register(Register::from_encoding(base).unwrap())
                                );
                                assert_eq!(
                                    site.operand().index,
                                    Some(Register::from_encoding(index).unwrap())
                                );
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                            }
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x67, 0x48, 0x8b, 0x34, 0x76], // address32 RSI data is outside this profile
            vec![0x48, 0x8b, 0x04, 0x66],       // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x35],       // no-base SIB lacks its disp32
            vec![0x48, 0x8b, 0x04],             // truncated SIB
        ] {
            let code = CodeView::new(0x5b00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
        }
        for full in [
            vec![0x48, 0x8b, 0x04, 0x76],       // mod00, four bytes
            vec![0x48, 0x8b, 0x44, 0x36, 0xff], // mod01, five bytes
            vec![0x48, 0x8b, 0x84, 0x36, 0xff, 0xff, 0xff, 0x7f], // mod10, eight bytes
        ] {
            for length in 1..full.len() {
                let code = CodeView::new(0x5b00, &full[..length]).unwrap();
                assert!(compile_memory_load_block_v2(code, 0x5b00).is_err());
                assert!(compile_memory_store_block_v2(code, 0x5b00).is_err());
                assert!(compile_ir_memory_load_block_v2(code, 0x5b00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5b00).is_err());
            }
        }
    }

    #[test]
    fn address32_rsi_rdi_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for index in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            for &(mode, displacement) in &displacement_cases {
                                let sib = (scale_shift << 6) | (index << 3) | base;
                                let mut source =
                                    vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | 4, sib];
                                match mode {
                                    0 => {}
                                    1 => source.push(displacement as u8),
                                    _ => source.extend_from_slice(&displacement.to_le_bytes()),
                                }
                                let code = CodeView::new(0x5c00, &source).unwrap();
                                let mut decoder_memory = CodeMemory::full_code(code);
                                let decoded = long_decode::decode(
                                    &mut decoder_memory,
                                    0x5c00,
                                    AccessContext::default(),
                                )
                                .unwrap();
                                match operation {
                                    MemorySiteOperationV2::Load => {
                                        assert!(crate::block::is_checked_qword_load_form(decoded));
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x5c00).unwrap();
                                        assert!(
                                            direct.site().matches_source_bytes(&source, operation)
                                        );
                                        let ir =
                                            compile_ir_memory_load_block_v2(code, 0x5c00).unwrap();
                                        assert_eq!(ir.source_bytes(), source);
                                        assert!(ir.site().has_supported_profile());
                                    }
                                    MemorySiteOperationV2::Store => {
                                        assert!(crate::block::is_checked_qword_store_form(decoded));
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x5c00).unwrap();
                                        assert!(
                                            direct.site().matches_source_bytes(&source, operation)
                                        );
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x5c00).unwrap();
                                        assert_eq!(ir.source_bytes(), source);
                                        assert!(ir.site().has_supported_profile());
                                    }
                                }
                                let site = match operation {
                                    MemorySiteOperationV2::Load => {
                                        compile_ir_memory_load_block_v2(code, 0x5c00)
                                            .unwrap()
                                            .site()
                                            .clone()
                                    }
                                    MemorySiteOperationV2::Store => {
                                        compile_ir_memory_store_block_v2(code, 0x5c00)
                                            .unwrap()
                                            .site()
                                            .clone()
                                    }
                                };
                                assert!(site.matches_source_bytes(&source, operation));
                                assert_eq!(site.next_rip(), 0x5c00 + source.len() as u64);
                                assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand.base,
                                    AddressBase::Register(Register::from_encoding(base).unwrap())
                                );
                                assert_eq!(
                                    site.operand.index,
                                    Some(Register::from_encoding(index).unwrap())
                                );
                                assert_eq!(site.operand.scale, 1 << scale_shift);
                                assert_eq!(site.operand.displacement, i64::from(displacement));
                            }
                        }
                    }
                }
            }
        }

        for opcode in [0x8b_u8, 0x89] {
            for (mode, displacement) in [(0_u8, 0_i32), (1, i8::MIN as i32), (2, i32::MIN)] {
                let source = {
                    let mut bytes = vec![0x67, 0x48, opcode, (mode << 6) | 4, 0x76];
                    match mode {
                        0 => {}
                        1 => bytes.push(displacement as u8),
                        _ => bytes.extend_from_slice(&displacement.to_le_bytes()),
                    }
                    bytes
                };
                for visible in 1..source.len() {
                    let code = CodeView::new(0x5c00, &source[..visible]).unwrap();
                    assert!(compile_memory_load_block_v2(code, 0x5c00).is_err());
                    assert!(compile_memory_store_block_v2(code, 0x5c00).is_err());
                    assert!(compile_ir_memory_load_block_v2(code, 0x5c00).is_err());
                    assert!(compile_ir_memory_store_block_v2(code, 0x5c00).is_err());
                }
            }
        }

        for (full, operation) in [
            (
                vec![0x67, 0x48, 0x8b, 0x34, 0x36],
                MemorySiteOperationV2::Load,
            ),
            (
                vec![0x67, 0x48, 0x89, 0x34, 0x36],
                MemorySiteOperationV2::Store,
            ),
        ] {
            for length in 1..full.len() {
                let code = CodeView::new(0x5c00, &full[..length]).unwrap();
                assert!(compile_ir_memory_load_block_v2(code, 0x5c00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5c00).is_err());
            }
            let code = CodeView::new(0x5c00, &full).unwrap();
            assert!(
                match operation {
                    MemorySiteOperationV2::Load =>
                        compile_memory_load_block_v2(code, 0x5c00).is_err(),
                    MemorySiteOperationV2::Store =>
                        compile_memory_store_block_v2(code, 0x5c00).is_err(),
                },
                "unexpectedly accepted {full:02x?}"
            );
        }
    }

    #[test]
    fn address32_rsi_rdi_base_rbp_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for scale_shift in 0..4_u8 {
                    for data in 0..6_u8 {
                        for &(mode, displacement) in &displacement_cases {
                            let sib = (scale_shift << 6) | (5 << 3) | base;
                            let mut source =
                                vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | 4, sib];
                            match mode {
                                0 => {}
                                1 => source.push(displacement as u8),
                                _ => source.extend_from_slice(&displacement.to_le_bytes()),
                            }
                            let code = CodeView::new(0x5c00, &source).unwrap();
                            let mut decoder_memory = CodeMemory::full_code(code);
                            let decoded = long_decode::decode(
                                &mut decoder_memory,
                                0x5c00,
                                AccessContext::default(),
                            )
                            .unwrap();
                            match operation {
                                MemorySiteOperationV2::Load => {
                                    assert!(crate::block::is_checked_qword_load_form(decoded));
                                    crate::block_compile::compile_checked_load_block_v2(
                                        code, 0x5c00,
                                    )
                                    .unwrap_or_else(|error| {
                                        panic!("checked {source:02x?}: {error:?}")
                                    });
                                    let direct = compile_memory_load_block_v2(code, 0x5c00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_load_block_v2(code, 0x5c00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert!(direct.site().matches_source_bytes(&source, operation));
                                    assert!(ir.site().matches_source_bytes(&source, operation));
                                    assert_eq!(ir.source_bytes(), source);
                                    assert_eq!(direct.site().operand(), ir.site().operand());
                                }
                                MemorySiteOperationV2::Store => {
                                    assert!(crate::block::is_checked_qword_store_form(decoded));
                                    let direct = compile_memory_store_block_v2(code, 0x5c00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_store_block_v2(code, 0x5c00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert!(direct.site().matches_source_bytes(&source, operation));
                                    assert!(ir.site().matches_source_bytes(&source, operation));
                                    assert_eq!(ir.source_bytes(), source);
                                    assert_eq!(direct.site().operand(), ir.site().operand());
                                }
                            }
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    compile_memory_load_block_v2(code, 0x5c00)
                                        .unwrap()
                                        .site()
                                        .clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    compile_memory_store_block_v2(code, 0x5c00)
                                        .unwrap()
                                        .site()
                                        .clone()
                                }
                            };
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.instruction_index(), 0);
                            assert_eq!(site.next_rip(), 0x5c00 + source.len() as u64);
                            assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(site.operand().index, Some(Register::Rbp));
                            assert_eq!(site.operand().scale, 1 << scale_shift);
                            assert_eq!(site.operand().displacement, i64::from(displacement));
                        }
                    }
                }
            }
        }

        let invalid = [
            vec![0x67, 0x48, 0x8b, 0x34, 0x36], // RSI data is outside the whitelist
            vec![0x67, 0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0], // address32 no-index scale neighbor
            vec![0x66, 0x67, 0x48, 0x8b, 0x04, 0x2e], // extra prefix
        ];
        for source in invalid {
            let code = CodeView::new(0x5c00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5c00).is_err(),
                "{source:02x?}"
            );
        }
        for full in [
            vec![0x67, 0x48, 0x8b, 0x04, 0x2e],
            vec![0x67, 0x48, 0x8b, 0x44, 0x6e, 0xff],
            vec![0x67, 0x48, 0x8b, 0x84, 0xee, 0xff, 0xff, 0xff, 0x7f],
        ] {
            for length in 1..full.len() {
                let code = CodeView::new(0x5c00, &full[..length]).unwrap();
                assert!(compile_memory_load_block_v2(code, 0x5c00).is_err());
                assert!(compile_memory_store_block_v2(code, 0x5c00).is_err());
                assert!(compile_ir_memory_load_block_v2(code, 0x5c00).is_err());
                assert!(compile_ir_memory_store_block_v2(code, 0x5c00).is_err());
            }
        }
    }

    #[test]
    fn rsi_rdi_indexed_sib_signed_displacements_have_exact_qword_profile() {
        let displacement_cases = [
            (1_u8, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for index in 0..4_u8 {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            for (mode, displacement) in displacement_cases {
                                let mut source = vec![
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data << 3) | 4,
                                    (scale_shift << 6) | (index << 3) | base,
                                ];
                                match mode {
                                    1 => source.push(displacement as i8 as u8),
                                    2 => source.extend_from_slice(&displacement.to_le_bytes()),
                                    _ => unreachable!(),
                                }
                                let length = source.len() as u8;
                                let code = CodeView::new(0x5b00, &source).unwrap();
                                let mut decoder_memory = CodeMemory::full_code(code);
                                let decoded = long_decode::decode(
                                    &mut decoder_memory,
                                    0x5b00,
                                    AccessContext::default(),
                                )
                                .unwrap();
                                match operation {
                                    MemorySiteOperationV2::Load => {
                                        assert!(crate::block::is_checked_qword_load_form(decoded));
                                        compile_memory_load_block_v2(code, 0x5b00).unwrap();
                                        crate::block_compile::compile_checked_load_block_v2(
                                            code, 0x5b00,
                                        )
                                        .unwrap_or_else(|error| panic!("checked: {error:?}"));
                                        let ir = compile_ir_memory_load_block_v2(code, 0x5b00)
                                            .unwrap_or_else(|error| {
                                                panic!("{source:02x?}: {error:?}")
                                            });
                                        assert!(ir.site().matches_source_bytes(&source, operation));
                                    }
                                    MemorySiteOperationV2::Store => {
                                        assert!(crate::block::is_checked_qword_store_form(decoded));
                                        compile_memory_store_block_v2(code, 0x5b00).unwrap();
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x5b00).unwrap();
                                        assert!(ir.site().matches_source_bytes(&source, operation));
                                    }
                                }
                                assert!(crate::block::exact_direct_qword_memory_bytes(
                                    code, 0x5b00, length, opcode
                                ));
                            }
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x48, 0x8b, 0x44, 0x66, 0], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x84, 0x66, 0, 0, 0, 0], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x74, 0x25, 0], // RSI data is outside this profile
            vec![0x48, 0x8b, 0xb4, 0x25, 0, 0, 0, 0], // RSI data is outside this profile
            vec![0x48, 0x8b, 0x44, 0x0e],    // truncated disp8
            vec![0x48, 0x8b, 0x84, 0x0e, 0, 0, 0], // truncated disp32
        ] {
            let code = CodeView::new(0x5b00, &source).unwrap();
            assert!(
                compile_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_load_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
            assert!(
                compile_ir_memory_store_block_v2(code, 0x5b00).is_err(),
                "{source:02x?}"
            );
        }
    }

    #[test]
    fn ir_checked_sib_singletons_match_decoded_metadata_for_all_registers_and_scales() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for base in 0..4_u8 {
                    for index in 0..4_u8 {
                        for scale_bits in 0..4_u8 {
                            let source = [
                                0x48,
                                opcode,
                                (data << 3) | 4,
                                (scale_bits << 6) | (index << 3) | base,
                            ];
                            let code = CodeView::new(0x5600, &source).unwrap();
                            let (ir_site, ir_source, v2_site) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x5600).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x5600).unwrap();
                                    (
                                        ir.site().clone(),
                                        ir.source_bytes().to_vec(),
                                        direct.site().clone(),
                                    )
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x5600).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x5600).unwrap();
                                    (
                                        ir.site().clone(),
                                        ir.source_bytes().to_vec(),
                                        direct.site().clone(),
                                    )
                                }
                            };
                            assert_eq!(ir_site, v2_site);
                            assert_eq!(ir_source, source);
                            assert_eq!(ir_site.guest_rip(), 0x5600);
                            assert_eq!(ir_site.next_rip(), 0x5604);
                            assert_eq!(
                                ir_site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(ir_site.operand().index, Register::from_encoding(index));
                            assert_eq!(ir_site.operand().scale, 1_u8 << scale_bits);
                            assert_eq!(ir_site.operand().displacement, 0);
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x48, opcode, 0x04],             // truncated before SIB
                vec![0x48, opcode, 0x04, 0x60],       // no-index with non-unit scale
                vec![0x48, opcode, 0x04, 0x05],       // no-base
                vec![0x48, opcode, 0x04, 0x66],       // RSI base with no-index scale alias
                vec![0x4a, opcode, 0x04, 0x18],       // extended index
                vec![0x49, opcode, 0x04, 0x18],       // extended base
                vec![0x48, opcode, 0x04, 0x18, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x5700, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x5700)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5700).map(|_| ()))
                } else {
                    compile_ir_memory_store_block_v2(code, 0x5700)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5700).map(|_| ()))
                };
                assert!(result.is_err(), "unexpected admission: {source:02x?}");
            }
        }
    }

    #[test]
    fn ir_rsp_base_no_index_sib_is_exactly_the_0x24_form() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                let source = [0x48, opcode, (data << 3) | 4, 0x24];
                let code = CodeView::new(0x5710, &source).unwrap();
                let (site, v2_site, ir_source) = match operation {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x5710).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x5710).unwrap();
                        (
                            ir.site().clone(),
                            direct.site().clone(),
                            ir.source_bytes().to_vec(),
                        )
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x5710).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x5710).unwrap();
                        (
                            ir.site().clone(),
                            direct.site().clone(),
                            ir.source_bytes().to_vec(),
                        )
                    }
                };
                assert_eq!(site, v2_site);
                assert_eq!(ir_source, source);
                assert_eq!(site.guest_rip(), 0x5710);
                assert_eq!(site.next_rip(), 0x5714);
                assert_eq!(site.operand().base, AddressBase::Register(Register::Rsp));
                assert_eq!(site.operand().index, None);
                assert_eq!(site.operand().scale, 1);
                assert_eq!(site.operand().displacement, 0);
                assert!(site.matches_source_bytes(&source, operation));
            }

            for bytes in [
                vec![0x48, opcode, 0x04],       // truncated before SIB
                vec![0x48, opcode, 0x24, 0x25], // no base
                vec![0x48, opcode, 0x04, 0x64], // scale > 1, no index
                vec![0x48, opcode, 0x04, 0x25], // no base
                vec![0x67, 0x48, opcode, 0x34, 0x34],
                vec![0x48, 0x67, opcode, 0x04, 0x24],
                vec![0x48, opcode, 0x04, 0x24, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x5710, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5710)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5710).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5710)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5710).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rbp_data_with_rsp_sib_base_disp8_and_disp32_is_exact() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for (modrm, length, values) in [
                (
                    0x6c_u8,
                    5_usize,
                    vec![i8::MIN as i32, -1, 0, i8::MAX as i32],
                ),
                (0xac_u8, 8_usize, vec![i32::MIN, -1, 0, i32::MAX, 128]),
            ] {
                for displacement in values {
                    let bytes = displacement.to_le_bytes();
                    let mut source = vec![0x48, opcode, modrm, 0x24];
                    source.extend_from_slice(&bytes[..length - 4]);
                    assert_eq!(source.len(), length);
                    let code = CodeView::new(0x5730, &source).unwrap();
                    let (site, direct_site, copied) = match operation {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x5730).unwrap();
                            let direct = compile_memory_load_block_v2(code, 0x5730).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x5730).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x5730).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                    };
                    assert_eq!(site, direct_site);
                    assert_eq!(copied, source);
                    assert_eq!(
                        site.destination(),
                        (operation == MemorySiteOperationV2::Load).then_some(5)
                    );
                    assert_eq!(
                        site.source(),
                        (operation == MemorySiteOperationV2::Store).then_some(5)
                    );
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rsp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, i64::from(displacement));
                    assert_eq!(site.next_rip(), 0x5730 + length as u64);
                    assert!(site.matches_source_bytes(&source, operation));
                }

                let valid = if length == 5 {
                    vec![0x48, opcode, 0x6c, 0x24, 0]
                } else {
                    vec![0x48, opcode, 0xac, 0x24, 0, 0, 0, 0]
                };
                for partial in 1..length {
                    let code = CodeView::new(0x5730, &valid[..partial]).unwrap();
                    let result = match operation {
                        MemorySiteOperationV2::Load => {
                            compile_ir_memory_load_block_v2(code, 0x5730).and_then(|_| {
                                compile_memory_load_block_v2(code, 0x5730).map(|_| ())
                            })
                        }
                        MemorySiteOperationV2::Store => {
                            compile_ir_memory_store_block_v2(code, 0x5730).and_then(|_| {
                                compile_memory_store_block_v2(code, 0x5730).map(|_| ())
                            })
                        }
                    };
                    assert!(result.is_err(), "partial len={partial}");
                }
                let mut wrong_sib = valid.clone();
                wrong_sib[3] = 0x65; // RBP base, no index, non-unit scale.
                let mut wrong_modrm = valid.clone();
                wrong_modrm[2] = if length == 5 { 0x70 } else { 0xb0 };
                let mut wrong_prefix = vec![0x64];
                wrong_prefix.extend_from_slice(&valid);
                let mut reordered_prefix = vec![0x48, 0x67, opcode];
                reordered_prefix.extend_from_slice(&valid[2..]);
                for invalid in [wrong_sib, wrong_modrm, wrong_prefix, reordered_prefix] {
                    let code = CodeView::new(0x5730, &invalid).unwrap();
                    let result = match operation {
                        MemorySiteOperationV2::Load => {
                            compile_ir_memory_load_block_v2(code, 0x5730).and_then(|_| {
                                compile_memory_load_block_v2(code, 0x5730).map(|_| ())
                            })
                        }
                        MemorySiteOperationV2::Store => {
                            compile_ir_memory_store_block_v2(code, 0x5730).and_then(|_| {
                                compile_memory_store_block_v2(code, 0x5730).map(|_| ())
                            })
                        }
                    };
                    assert!(result.is_err(), "unexpectedly admitted {invalid:02x?}");
                }
            }
        }
    }

    #[test]
    fn ir_rbp_data_with_rsp_sib_base_is_only_exact_0x2c_24_form() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            let source = [0x48, opcode, 0x2c, 0x24];
            let code = CodeView::new(0x5718, &source).unwrap();
            let mut decoder_memory = CodeMemory::full_code(code);
            let decoded =
                long_decode::decode(&mut decoder_memory, 0x5718, AccessContext::default()).unwrap();
            if operation == MemorySiteOperationV2::Load {
                assert!(crate::block::is_checked_qword_load_form(decoded));
            } else {
                assert!(crate::block::is_checked_qword_store_form(decoded));
            }
            assert!(crate::block::exact_direct_qword_memory_bytes(
                code, 0x5718, 4, opcode
            ));
            if operation == MemorySiteOperationV2::Load {
                let (site_probe, bytes_probe) = checked_load_metadata_v2(code, 0x5718).unwrap();
                assert!(site_probe.has_supported_profile());
                assert!(site_probe.matches_source_bytes(bytes_probe, MemorySiteOperationV2::Load));
            }
            let (ir_site, direct_site, copied) = match operation {
                MemorySiteOperationV2::Load => {
                    let ir = compile_ir_memory_load_block_v2(code, 0x5718).unwrap();
                    let direct = compile_memory_load_block_v2(code, 0x5718).unwrap();
                    (
                        ir.site().clone(),
                        direct.site().clone(),
                        ir.source_bytes().to_vec(),
                    )
                }
                MemorySiteOperationV2::Store => {
                    let ir = compile_ir_memory_store_block_v2(code, 0x5718).unwrap();
                    let direct = compile_memory_store_block_v2(code, 0x5718).unwrap();
                    (
                        ir.site().clone(),
                        direct.site().clone(),
                        ir.source_bytes().to_vec(),
                    )
                }
            };
            assert_eq!(ir_site, direct_site);
            assert_eq!(copied, source);
            assert_eq!(
                ir_site.destination(),
                (operation == MemorySiteOperationV2::Load).then_some(5)
            );
            assert_eq!(
                ir_site.source(),
                (operation == MemorySiteOperationV2::Store).then_some(5)
            );
            assert_eq!(ir_site.operand().base, AddressBase::Register(Register::Rsp));
            assert_eq!(ir_site.operand().index, None);
            assert_eq!(ir_site.operand().scale, 1);
            assert_eq!(ir_site.operand().displacement, 0);
            assert_eq!(ir_site.next_rip(), 0x571c);
            assert!(ir_site.has_supported_profile());
            assert!(ir_site.matches_source_bytes(&source, operation));

            for invalid in [
                vec![0x48, opcode, 0x2c],                   // truncated SIB
                vec![0x48, opcode, 0x2c, 0x2d],             // no-base RBP index
                vec![0x48, opcode, 0x2c, 0x64],             // non-unit scale
                vec![0x48, opcode, 0x34, 0x20],             // indexed address
                vec![0x48, opcode, 0x7c, 0x20, 0],          // indexed disp8 neighbor
                vec![0x48, opcode, 0xbc, 0x20, 0, 0, 0, 0], // indexed disp32 neighbor
                vec![0x67, 0x48, opcode, 0x34, 0x34],
                vec![0x48, 0x67, opcode, 0x2c, 0x24],
            ] {
                let code = CodeView::new(0x5718, &invalid).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5718)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5718).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5718)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5718).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {invalid:02x?}");
            }
        }
    }

    #[test]
    fn ir_rsp_data_and_base_is_only_exact_0x24_sib_form() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            let source = [0x48, opcode, 0x24, 0x24];
            let code = CodeView::new(0x5718, &source).unwrap();
            let (ir_site, direct_site, ir_source) = match operation {
                MemorySiteOperationV2::Load => {
                    let ir = compile_ir_memory_load_block_v2(code, 0x5718).unwrap();
                    let direct = compile_memory_load_block_v2(code, 0x5718).unwrap();
                    (
                        ir.site().clone(),
                        direct.site().clone(),
                        ir.source_bytes().to_vec(),
                    )
                }
                MemorySiteOperationV2::Store => {
                    let ir = compile_ir_memory_store_block_v2(code, 0x5718).unwrap();
                    let direct = compile_memory_store_block_v2(code, 0x5718).unwrap();
                    (
                        ir.site().clone(),
                        direct.site().clone(),
                        ir.source_bytes().to_vec(),
                    )
                }
            };
            assert_eq!(ir_site, direct_site);
            assert_eq!(ir_source, source);
            assert_eq!(ir_site.destination().or(ir_site.source()), Some(4));
            assert_eq!(ir_site.operand().base, AddressBase::Register(Register::Rsp));
            assert_eq!(ir_site.operand().index, None);
            assert_eq!(ir_site.operand().scale, 1);
            assert_eq!(ir_site.operand().displacement, 0);
            assert!(ir_site.matches_source_bytes(&source, operation));

            for bytes in [
                vec![0x48, opcode, 0x24],             // missing SIB
                vec![0x48, opcode, 0x24, 0x25],       // no base
                vec![0x48, opcode, 0x24, 0x64],       // non-unit scale with no index
                vec![0x48, opcode, 0x24, 0x2d],       // no-base RBP index
                vec![0x48, opcode, 0x24, 0x24, 0],    // displacement/trailing byte
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x48, 0x67, opcode, 0x24, 0x24], // reordered prefix
            ] {
                let code = CodeView::new(0x5718, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5718)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5718).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5718)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5718).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rsp_data_with_rbp_base_disp8_and_disp32_is_exact() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for (modrm, length, values) in [
                (
                    0x65_u8,
                    4_usize,
                    vec![i8::MIN as i32, -1, 0, i8::MAX as i32],
                ),
                (0xa5_u8, 7_usize, vec![i32::MIN, -1, 0, i32::MAX, 128]),
            ] {
                for displacement in values {
                    let bytes = displacement.to_le_bytes();
                    let mut source = vec![0x48, opcode, modrm];
                    source.extend_from_slice(&bytes[..length - 3]);
                    let code = CodeView::new(0x5740, &source).unwrap();
                    let (site, direct_site, copied) = match operation {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x5740).unwrap();
                            let direct = compile_memory_load_block_v2(code, 0x5740).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x5740).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x5740).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                    };
                    assert_eq!(site, direct_site);
                    assert_eq!(copied, source);
                    assert_eq!(
                        site.destination(),
                        (operation == MemorySiteOperationV2::Load).then_some(4)
                    );
                    assert_eq!(
                        site.source(),
                        (operation == MemorySiteOperationV2::Store).then_some(4)
                    );
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rbp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, i64::from(displacement));
                    assert_eq!(site.next_rip(), 0x5740 + length as u64);
                    assert!(site.matches_source_bytes(&source, operation));
                }
                let valid = if length == 4 {
                    vec![0x48, opcode, 0x65, 0]
                } else {
                    vec![0x48, opcode, 0xa5, 0, 0, 0, 0]
                };
                for partial in 1..length {
                    let code = CodeView::new(0x5740, &valid[..partial]).unwrap();
                    let result = match operation {
                        MemorySiteOperationV2::Load => {
                            compile_ir_memory_load_block_v2(code, 0x5740).and_then(|_| {
                                compile_memory_load_block_v2(code, 0x5740).map(|_| ())
                            })
                        }
                        MemorySiteOperationV2::Store => {
                            compile_ir_memory_store_block_v2(code, 0x5740).and_then(|_| {
                                compile_memory_store_block_v2(code, 0x5740).map(|_| ())
                            })
                        }
                    };
                    assert!(result.is_err(), "partial len={partial}");
                }
                let mut wrong_modrm = valid.clone();
                wrong_modrm[2] = if length == 4 { 0x64 } else { 0xa4 };
                let mut wrong_prefix = vec![0x64, 0x67];
                wrong_prefix.extend_from_slice(&valid);
                let mut reordered_prefix = vec![0x48, 0x67, opcode];
                reordered_prefix.extend_from_slice(&valid[2..]);
                let indexed_sib = if length == 4 {
                    vec![0x48, opcode, 0x74, 0x25, 0]
                } else {
                    vec![0x48, opcode, 0xb4, 0x25, 0, 0, 0, 0]
                };
                let mut segment_prefix = vec![0x64, 0x48];
                segment_prefix.extend_from_slice(&valid[1..]);
                for invalid in [
                    wrong_modrm,
                    wrong_prefix,
                    reordered_prefix,
                    indexed_sib,
                    segment_prefix,
                ] {
                    let code = CodeView::new(0x5740, &invalid).unwrap();
                    let result = match operation {
                        MemorySiteOperationV2::Load => {
                            compile_ir_memory_load_block_v2(code, 0x5740).and_then(|_| {
                                compile_memory_load_block_v2(code, 0x5740).map(|_| ())
                            })
                        }
                        MemorySiteOperationV2::Store => {
                            compile_ir_memory_store_block_v2(code, 0x5740).and_then(|_| {
                                compile_memory_store_block_v2(code, 0x5740).map(|_| ())
                            })
                        }
                    };
                    assert!(result.is_err(), "unexpectedly admitted {invalid:02x?}");
                }
            }
        }
    }

    #[test]
    fn ir_rsp_rbp_data_with_low_direct_base_displacements_is_exact() {
        let bases = [Register::Rax, Register::Rcx, Register::Rdx, Register::Rbx];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..6 {
                for (mode, length, values) in [
                    (1_u8, 4_usize, vec![i8::MIN as i32, -1, 0, i8::MAX as i32]),
                    (2_u8, 7_usize, vec![i32::MIN, -1, 0, i32::MAX, 128]),
                ] {
                    for base in bases {
                        for displacement in &values {
                            let modrm = mode << 6 | data_register << 3 | base as u8;
                            let disp_bytes = displacement.to_le_bytes();
                            let mut source = vec![0x48, opcode, modrm];
                            source.extend_from_slice(&disp_bytes[..length - 3]);
                            let code = CodeView::new(0x5750, &source).unwrap();
                            let (site, direct_site, copied) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x5750).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x5750).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x5750).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x5750).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                            };
                            assert_eq!(site, direct_site);
                            assert_eq!(copied, source);
                            assert_eq!(site.operand().base, AddressBase::Register(base));
                            assert_eq!(site.operand().index, None);
                            assert_eq!(site.operand().scale, 1);
                            assert_eq!(site.operand().displacement, i64::from(*displacement));
                            assert_eq!(site.next_rip(), 0x5750 + length as u64);
                            assert_eq!(
                                site.destination(),
                                (operation == MemorySiteOperationV2::Load).then_some(data_register)
                            );
                            assert_eq!(
                                site.source(),
                                (operation == MemorySiteOperationV2::Store)
                                    .then_some(data_register)
                            );
                            assert!(site.matches_source_bytes(&source, operation));
                        }
                    }
                }
            }
        }
        for (opcode, reg) in [(0x8b_u8, 4_u8), (0x89, 5)] {
            for (mode, length) in [(1_u8, 4_usize), (2, 7)] {
                let mut invalid = vec![0x48, opcode, mode << 6 | reg << 3 | 4];
                invalid.resize(length, 0);
                let code = CodeView::new(0x5750, &invalid).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x5750).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x5750).map(|_| ())
                };
                assert!(result.is_err(), "SIB unexpectedly admitted {invalid:02x?}");
            }
        }
        for bytes in [
            vec![0x67, 0x48, 0x8b, 0x64, 0], // incomplete SIB
            vec![0x48, 0x67, 0x89, 0xa8, 0, 0, 0, 0],
            vec![0x49, 0x8b, 0x7d, 0], // extended base
            vec![0x49, 0x89, 0x75, 0], // extended base
        ] {
            let code = CodeView::new(0x5750, &bytes).unwrap();
            let result = if bytes[1] == 0x8b || bytes[2] == 0x8b {
                compile_ir_memory_load_block_v2(code, 0x5750).map(|_| ())
            } else {
                compile_ir_memory_store_block_v2(code, 0x5750).map(|_| ())
            };
            assert!(result.is_err(), "unsupported form admitted {bytes:02x?}");
        }
    }

    #[test]
    fn ir_rsp_data_and_base_disp8_is_exact_signed_0x64_sib_form() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for displacement in [i8::MIN, -1, 0, i8::MAX] {
                let source = [0x48, opcode, 0x64, 0x24, displacement as u8];
                let code = CodeView::new(0x5730, &source).unwrap();
                let (ir_site, direct_site, ir_source) = match operation {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x5730).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x5730).unwrap();
                        (
                            ir.site().clone(),
                            direct.site().clone(),
                            ir.source_bytes().to_vec(),
                        )
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x5730).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x5730).unwrap();
                        (
                            ir.site().clone(),
                            direct.site().clone(),
                            ir.source_bytes().to_vec(),
                        )
                    }
                };
                assert_eq!(ir_site, direct_site);
                assert_eq!(ir_source, source);
                assert_eq!(ir_site.destination().or(ir_site.source()), Some(4));
                assert_eq!(ir_site.next_rip(), 0x5735);
                assert_eq!(ir_site.operand().base, AddressBase::Register(Register::Rsp));
                assert_eq!(ir_site.operand().index, None);
                assert_eq!(ir_site.operand().scale, 1);
                assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                assert!(ir_site.matches_source_bytes(&source, operation));
            }

            for bytes in [
                vec![0x48, opcode, 0x64, 0x24],          // truncated displacement
                vec![0x48, opcode, 0x74, 0x25, 0],       // RSI data is outside this profile
                vec![0x48, opcode, 0x64, 0x64, 0],       // scale without index
                vec![0x67, 0x48, opcode, 0x74, 0x34, 0], // unsupported RSI data register
                vec![0x48, 0x67, opcode, 0x64, 0x24, 0], // reordered prefix
                vec![0x48, opcode, 0x64, 0x24, 0, 0x90], // trailing instruction
            ] {
                let code = CodeView::new(0x5730, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5730)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5730).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5730)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5730).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rsp_data_and_base_disp32_is_exact_full_source_0xa4_sib_form() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for displacement in [i32::MIN, -1, 0, i32::MAX] {
                let displacement_bytes = displacement.to_le_bytes();
                let source = [
                    0x48,
                    opcode,
                    0xa4,
                    0x24,
                    displacement_bytes[0],
                    displacement_bytes[1],
                    displacement_bytes[2],
                    displacement_bytes[3],
                ];
                let code = CodeView::new(0x5740, &source).unwrap();
                let (ir_site, direct_site, ir_source) = match operation {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x5740).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x5740).unwrap();
                        (
                            ir.site().clone(),
                            direct.site().clone(),
                            ir.source_bytes().to_vec(),
                        )
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x5740).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x5740).unwrap();
                        (
                            ir.site().clone(),
                            direct.site().clone(),
                            ir.source_bytes().to_vec(),
                        )
                    }
                };
                assert_eq!(ir_site, direct_site);
                assert_eq!(ir_source, source);
                assert_eq!(ir_site.destination().or(ir_site.source()), Some(4));
                assert_eq!(ir_site.next_rip(), 0x5748);
                assert_eq!(ir_site.operand().base, AddressBase::Register(Register::Rsp));
                assert_eq!(ir_site.operand().index, None);
                assert_eq!(ir_site.operand().scale, 1);
                assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                assert!(ir_site.matches_source_bytes(&source, operation));
            }

            for partial_len in 1..8 {
                let source = [0x48, opcode, 0xa4, 0x24, 0xff, 0xff, 0xff, 0x7f];
                let code = CodeView::new(0x5740, &source[..partial_len]).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5740)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5740).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5740)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5740).map(|_| ())),
                };
                assert!(result.is_err(), "partial len={partial_len}");
            }

            for source in [
                [0x48, opcode, 0xb4, 0x25, 0, 0, 0, 0], // RSI data is outside this profile
                [0x48, opcode, 0xa4, 0x64, 0, 0, 0, 0], // scale without index
                [0x67, 0x48, opcode, 0xa4, 0x24, 0, 0, 0],
            ] {
                let code = CodeView::new(0x5740, &source).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5740)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5740).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5740)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5740).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {source:02x?}");
            }
        }

        let first = [0x48, 0x8b, 0xa4, 0x24, 0, 0, 0, 0x01];
        let second = [0x48, 0x8b, 0xa4, 0x24, 0, 0, 0, 0x02];
        let first_block =
            compile_ir_memory_load_block_v2(CodeView::new(0x5740, &first).unwrap(), 0x5740)
                .unwrap();
        let second_block =
            compile_ir_memory_load_block_v2(CodeView::new(0x5740, &second).unwrap(), 0x5740)
                .unwrap();
        assert_eq!(first_block.source_bytes(), &first);
        assert_eq!(second_block.source_bytes(), &second);
        assert_ne!(first_block.source_bytes(), second_block.source_bytes());
        assert_eq!(first_block.site().operand().displacement, 0x0100_0000);
        assert_eq!(second_block.site().operand().displacement, 0x0200_0000);
    }

    #[test]
    fn ir_rsp_base_no_index_sib_disp8_is_exact_and_signed() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for displacement in [i8::MIN, -1, 0, i8::MAX] {
                    let source = [0x48, opcode, 0x44 | (data << 3), 0x24, displacement as u8];
                    let code = CodeView::new(0x5720, &source).unwrap();
                    let (site, v2_site, ir_source) = match operation {
                        MemorySiteOperationV2::Load => {
                            let direct = compile_memory_load_block_v2(code, 0x5720).unwrap();
                            let built = crate::block::build_checked_qword_load_block(
                                code,
                                0x5720,
                                od_isa::memory::AccessContext::default(),
                                crate::block::BlockLimits::default(),
                            )
                            .unwrap();
                            let lowered = crate::lowering::lower(&built);
                            assert!(
                                lowered.is_ok(),
                                "lowering failed for {source:02x?}: {lowered:?}"
                            );
                            let ir = compile_ir_memory_load_block_v2(code, 0x5720).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x5720).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x5720).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                    };
                    assert_eq!(site, v2_site);
                    assert_eq!(ir_source, source);
                    assert_eq!(site.guest_rip(), 0x5720);
                    assert_eq!(site.next_rip(), 0x5725);
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rsp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, i64::from(displacement));
                    assert!(site.matches_source_bytes(&source, operation));
                }
            }

            for bytes in [
                vec![0x48, opcode, 0x44, 0x24],    // truncated before disp8
                vec![0x48, opcode, 0x74, 0x25, 0], // RSI data is outside this profile
                vec![0x48, opcode, 0x44, 0x64, 0], // scale != 1, no index
                vec![0x67, 0x48, opcode, 0x74, 0x34, 0],
                vec![0x48, 0x67, opcode, 0x44, 0x24, 0],
                vec![0x48, opcode, 0x44, 0x24, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x5720, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5720)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5720).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5720)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5720).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rsp_base_no_index_sib_disp32_is_exact_and_keeps_all_source_bytes() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for displacement in [i32::MIN, -1, 0, i32::MAX] {
                    let source = [
                        0x48,
                        opcode,
                        0x84 | (data << 3),
                        0x24,
                        displacement as u8,
                        (displacement >> 8) as u8,
                        (displacement >> 16) as u8,
                        (displacement >> 24) as u8,
                    ];
                    let code = CodeView::new(0x5730, &source).unwrap();
                    let (site, direct_site, stored_source) = match operation {
                        MemorySiteOperationV2::Load => {
                            let direct = compile_memory_load_block_v2(code, 0x5730).unwrap();
                            let ir = compile_ir_memory_load_block_v2(code, 0x5730).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                        MemorySiteOperationV2::Store => {
                            let direct = compile_memory_store_block_v2(code, 0x5730).unwrap();
                            let ir = compile_ir_memory_store_block_v2(code, 0x5730).unwrap();
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                    };
                    assert_eq!(site, direct_site);
                    assert_eq!(stored_source, source);
                    assert_eq!(site.guest_rip(), 0x5730);
                    assert_eq!(site.next_rip(), 0x5738);
                    assert_eq!(site.operation(), operation);
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rsp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, i64::from(displacement));
                    assert!(site.matches_source_bytes(&source, operation));
                }
            }

            // The last source byte is part of disp32 and must distinguish
            // otherwise identical instruction identities and decoded sites.
            let source_a = [0x48, opcode, 0x84, 0x24, 0, 0, 0, 1];
            let source_b = [0x48, opcode, 0x84, 0x24, 0, 0, 0, 2];
            let code_a = CodeView::new(0x5730, &source_a).unwrap();
            let code_b = CodeView::new(0x5730, &source_b).unwrap();
            let site_a = match operation {
                MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code_a, 0x5730)
                    .unwrap()
                    .site()
                    .clone(),
                MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code_a, 0x5730)
                    .unwrap()
                    .site()
                    .clone(),
            };
            let site_b = match operation {
                MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code_b, 0x5730)
                    .unwrap()
                    .site()
                    .clone(),
                MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code_b, 0x5730)
                    .unwrap()
                    .site()
                    .clone(),
            };
            assert_eq!(site_a.operand().displacement, 0x0100_0000);
            assert_eq!(site_b.operand().displacement, 0x0200_0000);
            assert_ne!(site_a, site_b);

            for bytes in [
                vec![0x48, opcode, 0x84, 0x24, 0, 0, 0], // truncated disp32
                vec![0x48, opcode, 0xb4, 0x25, 0, 0, 0, 0], // RSI data is outside this profile
                vec![0x48, opcode, 0x84, 0x24, 0, 0, 0, 0, 0x90], // trailing byte
                vec![0x67, 0x48, opcode, 0xb4, 0x34, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x48, 0x67, opcode, 0x84, 0x24, 0, 0, 0, 0], // reordered prefix
                vec![0x64, 0x48, opcode, 0x84, 0x24, 0, 0, 0, 0], // segment prefix
                vec![0xf3, 0x48, opcode, 0x84, 0x24, 0, 0, 0, 0], // group-1 prefix
            ] {
                let code = CodeView::new(0x5730, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x5730)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x5730).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x5730)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x5730).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_checked_sib_displacements_match_decoded_metadata_and_reject_other_shapes() {
        let displacement_cases: &[(u8, i32)] = &[
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (4, i32::MIN),
            (4, -1),
            (4, 0),
            (4, i32::MAX),
        ];
        let register_cases = [(0_u8, 1_u8, 2_u8), (0, 0, 2), (0, 1, 0), (2, 3, 3)];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for (data, base, index) in register_cases {
                for scale_bits in 0..4_u8 {
                    for &(width, displacement) in displacement_cases {
                        let mode = if width == 1 { 1 } else { 2 };
                        let modrm = (mode << 6) | (data << 3) | 4;
                        let sib = (scale_bits << 6) | (index << 3) | base;
                        let mut bytes = vec![0x48, opcode, modrm, sib];
                        if width == 1 {
                            bytes.push(displacement as u8);
                        } else {
                            bytes.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let code = CodeView::new(0x5680, &bytes).unwrap();
                        let (ir_site, ir_source, v2_site) = match operation {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x5680).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x5680).unwrap();
                                (
                                    ir.site().clone(),
                                    ir.source_bytes().to_vec(),
                                    direct.site().clone(),
                                )
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x5680).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x5680).unwrap();
                                (
                                    ir.site().clone(),
                                    ir.source_bytes().to_vec(),
                                    direct.site().clone(),
                                )
                            }
                        };
                        assert_eq!(ir_site, v2_site);
                        assert_eq!(ir_source, bytes);
                        assert_eq!(ir_site.guest_rip(), 0x5680);
                        assert_eq!(ir_site.next_rip(), 0x5680 + bytes.len() as u64);
                        assert_eq!(
                            ir_site.operand().base,
                            AddressBase::Register(Register::from_encoding(base).unwrap())
                        );
                        assert_eq!(ir_site.operand().index, Register::from_encoding(index));
                        assert_eq!(ir_site.operand().scale, 1_u8 << scale_bits);
                        assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                        assert!(ir_site.matches_source_bytes(&bytes, operation));
                    }
                }
            }
        }

        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for bytes in [
                vec![0x48, opcode, 0x44],                         // truncated before SIB
                vec![0x48, opcode, 0x44, 0x60, 0],                // no-index with non-unit scale
                vec![0x48, opcode, 0x74, 0x25, 0], // RSI data is outside this profile
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // no-base with invalid no-index scale
                vec![0x48, opcode, 0x44, 0x18],    // missing disp8
                vec![0x48, opcode, 0x44, 0x18, 0, 0x90], // tail byte
                vec![0x48, opcode, 0x84, 0x18, 0, 0, 0], // partial disp32
                vec![0x48, opcode, 0x84, 0x18, 0, 0, 0, 0, 0x90], // tail byte
                vec![0x4a, opcode, 0x44, 0x18, 0], // extended index
                vec![0x49, opcode, 0x44, 0x18, 0], // extended base
                vec![0x48, opcode, 0x04, 0x18, 0, 0x90], // wrong instruction length
            ] {
                let code = CodeView::new(0x5690, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x5690).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x5690).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rsp_rbp_data_with_indexed_sib_keeps_exact_qword_profile() {
        let displacement_cases: &[(u8, i32)] = &[
            (0, 0),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (4, i32::MIN),
            (4, -1),
            (4, 0),
            (4, i32::MAX),
        ];
        let base_index_cases = [
            (0_u8, 1_u8),
            (1, 1),
            (2, 3),
            (4, 0),
            (4, 1),
            (4, 2),
            (4, 3),
            (5, 0),
            (5, 1),
            (5, 2),
            (5, 3),
        ];
        for (opcode, operation) in [
            (0x8b_u8, MemorySiteOperationV2::Load),
            (0x89_u8, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for (base, index) in base_index_cases {
                    if base < 4 && data_register < 4 {
                        continue;
                    }
                    for scale_bits in 0..4_u8 {
                        for &(width, displacement) in displacement_cases {
                            if base == 5 && width == 0 {
                                // mod=00/SIB.base=101 is the no-base encoding.
                                continue;
                            }
                            let mode = match width {
                                0 => 0,
                                1 => 1,
                                4 => 2,
                                _ => unreachable!(),
                            };
                            let modrm = (mode << 6) | (data_register << 3) | 4;
                            let sib = (scale_bits << 6) | (index << 3) | base;
                            let mut source = vec![0x48, opcode, modrm, sib];
                            match width {
                                0 => {}
                                1 => source.push(displacement as u8),
                                4 => source.extend_from_slice(&displacement.to_le_bytes()),
                                _ => unreachable!(),
                            }
                            let code = CodeView::new(0x56a0, &source).unwrap();
                            let (ir_site, direct_site, copied) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x56a0).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x56a0).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x56a0).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x56a0).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                            };
                            assert_eq!(ir_site, direct_site);
                            assert_eq!(copied, source);
                            assert_eq!(ir_site.next_rip(), 0x56a0 + source.len() as u64);
                            assert_eq!(
                                ir_site.destination(),
                                (operation == MemorySiteOperationV2::Load).then_some(data_register)
                            );
                            assert_eq!(
                                ir_site.source(),
                                (operation == MemorySiteOperationV2::Store)
                                    .then_some(data_register)
                            );
                            assert_eq!(
                                ir_site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(
                                ir_site.operand().index,
                                Some(Register::from_encoding(index).unwrap())
                            );
                            assert_eq!(ir_site.operand().scale, 1_u8 << scale_bits);
                            assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                            assert!(ir_site.has_supported_profile());
                            assert!(ir_site.matches_source_bytes(&source, operation));
                        }
                    }
                }
            }
        }

        for (opcode, operation) in [
            (0x8b_u8, MemorySiteOperationV2::Load),
            (0x89_u8, MemorySiteOperationV2::Store),
        ] {
            for bytes in [
                vec![0x48, opcode, 0x24],             // truncated before SIB
                vec![0x48, opcode, 0x24, 0x60],       // unsupported no-index scale
                vec![0x48, opcode, 0x24, 0x25],       // no base
                vec![0x48, opcode, 0x24, 0x05],       // RBP base special
                vec![0x4a, opcode, 0x24, 0x18],       // extended index
                vec![0x49, opcode, 0x24, 0x18],       // extended base
                vec![0x67, 0x48, opcode, 0x24, 0x60], // address32 no-index with non-unit scale
                vec![0x48, 0x67, opcode, 0x24, 0x20], // reordered prefix
            ] {
                let code = CodeView::new(0x56a0, &bytes).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x56a0)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x56a0).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x56a0)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x56a0).map(|_| ())),
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_checked_direct_displacements_reject_prefix_register_and_trailing_forms() {
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x48, opcode, 0xc3],             // mod=11
                vec![0x48, opcode, 0x44, 0x18],       // SIB with disp8
                vec![0x48, opcode, 0x43, 0x01, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x5500, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x5500).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x5500).map(|_| ())
                };
                assert!(result.is_err(), "bytes={bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_checked_store_has_non_replayable_write_to_commit_map() {
        let code = CodeView::new(0x1100, &[0x48, 0x89, 0x03]).unwrap();
        let ir_block = compile_ir_memory_store_block_v2(code, 0x1100).unwrap();
        let direct = compile_memory_store_block_v2(code, 0x1100).unwrap();
        assert_eq!(
            ir_block.compile_flavor(),
            crate::translation::CompileFlavor::S3_26bIrMemoryStoreV2
        );
        assert_eq!(ir_block.site(), direct.site());
        assert_eq!(ir_block.source_bytes(), &[0x48, 0x89, 0x03]);
        assert_eq!(ir_block.pc_map().instruction_count(), 1);
        assert!(ir_block.pc_map().ranges().iter().any(|r| r.phase
            == crate::statepoints::StatePhase::HelperCall
            && r.snapshot == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect));
        assert!(ir_block.pc_map().ranges().iter().any(|r| r.phase
            == crate::statepoints::StatePhase::AfterHelper
            && r.snapshot == crate::statepoints::SnapshotAdmissibility::InadmissibleAfterEffect));
        let helper = ir_block
            .pc_map()
            .ranges()
            .iter()
            .find(|r| r.phase == crate::statepoints::StatePhase::HelperCall)
            .unwrap();
        let after = ir_block
            .pc_map()
            .ranges()
            .iter()
            .find(|r| r.phase == crate::statepoints::StatePhase::AfterHelper)
            .unwrap();
        assert_ne!(
            ir_block
                .pc_map()
                .snapshot_admissibility_at(helper.range.start()),
            Some(crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction)
        );
        assert_ne!(
            ir_block
                .pc_map()
                .snapshot_admissibility_at(after.range.start()),
            Some(crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction)
        );
        let commit = ir_block
            .pc_map()
            .ranges()
            .iter()
            .find(|r| r.phase == crate::statepoints::StatePhase::CommitInProgress)
            .unwrap();
        assert!(
            ir_block
                .pc_map()
                .ranges()
                .iter()
                .filter(|r| r.range.start() >= helper.range.start()
                    && r.range.end() <= commit.range.end())
                .all(|r| r.snapshot
                    != crate::statepoints::SnapshotAdmissibility::ReplayCurrentInstruction)
        );
        assert_eq!(ir_block.pc_map().boundaries().len(), 1);
        assert_ne!(ir_block.code().words(), direct.code().words());
    }

    #[test]
    fn ir_checked_store_builder_accepts_sib_and_rejects_trailing_bytes() {
        let code = CodeView::new(0x1200, &[0x48, 0x89, 0x04, 0x18]).unwrap();
        let store = compile_ir_memory_store_block_v2(code, 0x1200).unwrap();
        assert_eq!(store.source_bytes(), &[0x48, 0x89, 0x04, 0x18]);

        let code = CodeView::new(0x1200, &[0x48, 0x89, 0x03, 0x90]).unwrap();
        assert!(matches!(
            compile_ir_memory_store_block_v2(code, 0x1200),
            Err(MemoryLoadCompileErrorV2::UnsupportedForm)
        ));
    }

    #[test]
    fn ir_rip_relative_singletons_reject_prefix_and_trailing_bytes() {
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0x05, 0, 0, 0, 0],
                vec![0x48, opcode, 0x05, 0, 0, 0, 0, 0x90],
            ] {
                let code = CodeView::new(0x5300, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x5300).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x5300).map(|_| ())
                };
                assert!(result.is_err(), "bytes={bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rip_relative_rsp_rbp_data_keeps_exact_next_rip_profile() {
        for (opcode, operation) in [
            (0x8b_u8, MemorySiteOperationV2::Load),
            (0x89_u8, MemorySiteOperationV2::Store),
        ] {
            for data_register in [4_u8, 5] {
                for displacement in [i32::MIN, -1, 0, 1, i32::MAX] {
                    let modrm = data_register << 3 | 5;
                    let mut instruction = vec![0x48, opcode, modrm];
                    instruction.extend_from_slice(&displacement.to_le_bytes());
                    let mut view_bytes = vec![0x90, 0x90];
                    view_bytes.extend_from_slice(&instruction);
                    let code = CodeView::new(0x5a00, &view_bytes).unwrap();
                    let entry = 0x5a02;
                    let (direct_site, shared_site, shared_bytes) = match operation {
                        MemorySiteOperationV2::Load => (
                            compile_memory_load_block_v2(code, entry)
                                .unwrap()
                                .site()
                                .clone(),
                            compile_ir_memory_load_block_v2(code, entry)
                                .unwrap()
                                .site()
                                .clone(),
                            compile_ir_memory_load_block_v2(code, entry)
                                .unwrap()
                                .source_bytes()
                                .to_vec(),
                        ),
                        MemorySiteOperationV2::Store => (
                            compile_memory_store_block_v2(code, entry)
                                .unwrap()
                                .site()
                                .clone(),
                            compile_ir_memory_store_block_v2(code, entry)
                                .unwrap()
                                .site()
                                .clone(),
                            compile_ir_memory_store_block_v2(code, entry)
                                .unwrap()
                                .source_bytes()
                                .to_vec(),
                        ),
                    };
                    assert_eq!(direct_site, shared_site);
                    assert_eq!(shared_bytes, instruction);
                    assert_eq!(shared_site.operand().base, AddressBase::NextRip);
                    assert_eq!(shared_site.operand().index, None);
                    assert_eq!(shared_site.operand().scale, 1);
                    assert_eq!(shared_site.operand().displacement, i64::from(displacement));
                    assert_eq!(shared_site.next_rip(), entry + 7);
                    assert_eq!(
                        shared_site.destination(),
                        (operation == MemorySiteOperationV2::Load).then_some(data_register)
                    );
                    assert_eq!(
                        shared_site.source(),
                        (operation == MemorySiteOperationV2::Store).then_some(data_register)
                    );
                    assert!(shared_site.has_supported_profile());
                    assert!(shared_site.matches_source_bytes(&instruction, operation));
                }
            }
        }

        for bytes in [
            vec![0x48, 0x8b, 0x25, 0, 0, 0], // truncated disp32
            vec![0x48, 0x89, 0x2d, 0, 0, 0],
            vec![0x67, 0x48, 0x8b, 0x25, 0, 0, 0, 0],
            vec![0x48, 0x89, 0x2d, 0, 0, 0, 0, 0x90], // trailing byte
            vec![0x48, 0x8b, 0x24, 0x65, 0, 0, 0, 0], // no-base with invalid no-index scale
        ] {
            let code = CodeView::new(0x5a00, &bytes).unwrap();
            let result = if bytes[1] == 0x8b || bytes[2] == 0x8b {
                compile_ir_memory_load_block_v2(code, 0x5a00).map(|_| ())
            } else {
                compile_ir_memory_store_block_v2(code, 0x5a00).map(|_| ())
            };
            assert!(result.is_err(), "unsupported bytes={bytes:02x?}");
        }
    }

    #[test]
    fn compiles_only_the_exact_checked_qword_load_form() {
        let bytes = [0x48, 0x8b, 0x00];
        let code = CodeView::new(0x1000, &bytes).unwrap();
        let block = compile_memory_load_block_v2(code, 0x1000).unwrap();
        assert_eq!(block.entry_rip(), 0x1000);
        assert_eq!(block.source_bytes(), &[0x48, 0x8b, 0x00]);
        assert_eq!(block.site().instruction_index(), 0);
        assert_eq!(block.site().guest_rip(), 0x1000);
        assert_eq!(block.site().next_rip(), 0x1003);
        assert_eq!(
            block.compile_flavor(),
            crate::translation::CompileFlavor::S3_19bMemoryLoadV2
        );
        assert!(!block.code().words().is_empty());
    }

    #[test]
    fn rejects_prefixes_displacements_and_out_of_profile_registers() {
        for bytes in [
            vec![0x40, 0x8b, 0x00],
            vec![0x48, 0x8b, 0xc0],       // register-register mod=11
            vec![0x48, 0x8b, 0x04],       // truncated SIB
            vec![0x48, 0x8b, 0x04, 0x66], // RSI base with no-index scale alias
            vec![0x48, 0x8b, 0x04, 0x60], // no-index SIB with non-unit scale
            vec![0x48, 0x8b, 0x04, 0x05], // no-base SIB requires disp32
            vec![0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0], // no-base with invalid no-index scale
            vec![0x48, 0x8b, 0x44, 0x60, 1], // no-index SIB with non-unit scale
            vec![0x48, 0x8b, 0x84, 0x60, 1, 0, 0, 0], // no-index SIB with non-unit scale
            vec![0x48, 0x8b, 0x44, 0x00], // mod=01 displacement
            vec![0x48, 0x8b, 0x40],       // truncated disp8
            vec![0x48, 0x8b, 0x80, 0, 0, 0], // truncated disp32
            vec![0x48, 0x8b, 0x05],       // truncated RIP-relative disp32
            vec![0x48, 0x8b, 0x05, 0, 0, 0, 0, 0x90], // RIP-relative tail
            vec![0x4a, 0x8b, 0x04, 0x00], // REX.X extended index
            vec![0x49, 0x8b, 0x04, 0x00], // REX.B extended base
            vec![0x67, 0x48, 0x8b, 0x00, 0x90], // address-size override tail
            vec![0x67, 0x48, 0x8b],       // truncated address-size override form
            vec![0x67, 0x48, 0x8b, 0x40], // address-size override mod01
            vec![0x67, 0x48, 0x8b, 0x80], // address-size override mod10
            vec![0x67, 0x48, 0x8b, 0xc0], // address-size override mod11
            vec![0x67, 0x48, 0x8b, 0x04], // address-size override SIB
            vec![0x67, 0x48, 0x8b, 0x05, 0, 0, 0, 0], // no-base disp32
            vec![0x67, 0x48, 0x8b, 0x75, 0], // unsupported data register
            vec![0x67, 0x49, 0x8b, 0x00], // extended base
            vec![0x48, 0x67, 0x8b, 0x00], // non-profile prefix order
            vec![0x66, 0x67, 0x48, 0x8b, 0x00], // extra operand-size prefix
            vec![0x48, 0x8b, 0x04, 0x00, 0x90], // ignored tail
            vec![0x48, 0x8b, 0x40, 0x00, 0x90], // disp8 form with tail
        ] {
            let code = CodeView::new(0x2000, &bytes).unwrap();
            assert!(
                matches!(
                    compile_memory_load_block_v2(code, 0x2000),
                    Err(MemoryLoadCompileErrorV2::UnsupportedForm)
                        | Err(MemoryLoadCompileErrorV2::Decode)
                ),
                "unexpectedly admitted bytes {bytes:02x?}"
            );
        }
    }

    #[test]
    fn admits_only_address_size_override_qword_mov_register_pairs() {
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for data_register in 0..4_u8 {
                for base in 0..4_u8 {
                    let bytes = [0x67, 0x48, opcode, (data_register << 3) | base];
                    let code = CodeView::new(0x2700, &bytes).unwrap();
                    let artifact = if is_load {
                        compile_memory_load_block_v2(code, 0x2700).unwrap().artifact
                    } else {
                        compile_memory_store_block_v2(code, 0x2700)
                            .unwrap()
                            .artifact
                    };
                    assert_eq!(&artifact.source_bytes[..4], &bytes);
                    assert_eq!(artifact.source_len, 4);
                    assert_eq!(artifact.site.guest_rip(), 0x2700);
                    assert_eq!(artifact.site.next_rip(), 0x2704);
                    assert_eq!(
                        artifact.site.operand.address_size,
                        od_isa::OperandSize::Dword
                    );
                    assert_eq!(artifact.site.operand.size, od_isa::OperandSize::Qword);
                    assert_eq!(
                        artifact.site.operand.base,
                        AddressBase::Register(Register::from_encoding(base).unwrap())
                    );
                    assert_eq!(artifact.site.operand.index, None);
                    assert_eq!(artifact.site.operand.scale, 1);
                    assert_eq!(artifact.site.operand.displacement, 0);
                    assert!(artifact.site.has_supported_profile());
                    assert!(artifact.site.matches_source_bytes(
                        &artifact.source_bytes[..4],
                        if is_load {
                            MemorySiteOperationV2::Load
                        } else {
                            MemorySiteOperationV2::Store
                        }
                    ));
                }
            }
        }
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0xc0],
                vec![0x67, 0x48, opcode, 0x04],
                vec![0x67, 0x48, opcode, 0x05, 0, 0, 0, 0],
                vec![0x67, 0x49, opcode, 0x00],
                vec![0x48, 0x67, opcode, 0x00],
                vec![0x67, 0x48, opcode],
                vec![0x67, 0x48, opcode, 0x00, 0x90],
            ] {
                let code = CodeView::new(0x2780, &bytes).unwrap();
                let result = if is_load {
                    compile_memory_load_block_v2(code, 0x2780).map(|_| ())
                } else {
                    compile_memory_store_block_v2(code, 0x2780).map(|_| ())
                };
                assert!(
                    matches!(
                        result,
                        Err(MemoryLoadCompileErrorV2::UnsupportedForm
                            | MemoryLoadCompileErrorV2::Decode)
                    ),
                    "unexpectedly admitted bytes {bytes:02x?}"
                );
            }
        }

        let address32_load = compile_memory_load_block_v2(
            CodeView::new(0x2790, &[0x67, 0x48, 0x8b, 0x00]).unwrap(),
            0x2790,
        )
        .unwrap();
        let address32_store = compile_memory_store_block_v2(
            CodeView::new(0x2790, &[0x67, 0x48, 0x89, 0x00]).unwrap(),
            0x2790,
        )
        .unwrap();
        assert!(
            !address32_load
                .site()
                .matches_source_bytes(&[0x48, 0x8b, 0x40, 0x00], MemorySiteOperationV2::Load)
        );
        assert!(
            !address32_store
                .site()
                .matches_source_bytes(&[0x48, 0x89, 0x40, 0x00], MemorySiteOperationV2::Store)
        );
        let legacy_load = compile_memory_load_block_v2(
            CodeView::new(0x2790, &[0x48, 0x8b, 0x00]).unwrap(),
            0x2790,
        )
        .unwrap();
        let legacy_store = compile_memory_store_block_v2(
            CodeView::new(0x2790, &[0x48, 0x89, 0x00]).unwrap(),
            0x2790,
        )
        .unwrap();
        assert!(
            !legacy_load
                .site()
                .matches_source_bytes(&[0x67, 0x48, 0x8b, 0x00], MemorySiteOperationV2::Load)
        );
        assert!(
            !legacy_store
                .site()
                .matches_source_bytes(&[0x67, 0x48, 0x89, 0x00], MemorySiteOperationV2::Store)
        );
    }

    #[test]
    fn ir_address32_direct_base_mov_has_exact_qword_descriptor() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for base in 0..4_u8 {
                    let bytes = [0x67, 0x48, opcode, (data << 3) | base];
                    let code = CodeView::new(0x27c0, &bytes).unwrap();
                    let (site, source) = match access {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x27c0).unwrap();
                            let direct = compile_memory_load_block_v2(code, 0x27c0).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x27c0).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x27c0).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                    };
                    assert_eq!(source, bytes);
                    assert_eq!(site.guest_rip(), 0x27c0);
                    assert_eq!(site.next_rip(), 0x27c4);
                    assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                    assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                    assert_eq!(
                        site.operand().base,
                        AddressBase::Register(Register::from_encoding(base).unwrap())
                    );
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, 0);
                    assert!(site.matches_source_bytes(&bytes, access));
                }
            }
        }

        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for bytes in [
                vec![0x67, 0x48, opcode],
                vec![0x67, 0x48, opcode, 0xc0],    // register form
                vec![0x67, 0x4c, opcode, 0],       // extended data/base
                vec![0x48, 0x67, opcode, 0],       // wrong prefix order
                vec![0x67, 0x48, opcode, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x27d0, &bytes).unwrap();
                let result = match access {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x27d0).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x27d0).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rbp_disp8_has_exact_qword_descriptor_and_rejects_other_rbp_forms() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for displacement in [i8::MIN, -1, 0, i8::MAX] {
                    let bytes = [0x48, opcode, 0x40 | (data << 3) | 5, displacement as u8];
                    let code = CodeView::new(0x2810, &bytes).unwrap();
                    let (site, source) = match access {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x2810).unwrap();
                            let direct = compile_memory_load_block_v2(code, 0x2810).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x2810).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x2810).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            (ir.site().clone(), ir.source_bytes().to_vec())
                        }
                    };
                    assert_eq!(source, bytes);
                    assert_eq!(site.guest_rip(), 0x2810);
                    assert_eq!(site.next_rip(), 0x2814);
                    assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand().base, AddressBase::Register(Register::Rbp));
                    assert_eq!(site.operand().index, None);
                    assert_eq!(site.operand().scale, 1);
                    assert_eq!(site.operand().displacement, i64::from(displacement));
                    assert!(site.matches_source_bytes(&bytes, access));
                }
            }

            for bytes in [
                vec![0x48, opcode, 0x45],          // missing disp8
                vec![0x48, opcode, 0x44, 0],       // SIB form
                vec![0x67, 0x48, opcode, 0x75, 0], // unsupported data register
                vec![0x4c, opcode, 0x45, 0],       // extended data register
                vec![0x48, opcode, 0x45, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x2810, &bytes).unwrap();
                let result = match access {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x2810).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x2810).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rbp_rbp_disp8_has_exact_dual_register_descriptor() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for displacement in [i8::MIN, -1, 0, i8::MAX] {
                let bytes = [0x48, opcode, 0x6d, displacement as u8];
                let code = CodeView::new(0x2820, &bytes).unwrap();
                let (site, source) = match access {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x2820).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x2820).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x2820).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x2820).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                };
                assert_eq!(source, bytes);
                assert_eq!(site.guest_rip(), 0x2820);
                assert_eq!(site.next_rip(), 0x2824);
                assert_eq!(site.operand().base, AddressBase::Register(Register::Rbp));
                assert_eq!(site.operand().index, None);
                assert_eq!(site.operand().scale, 1);
                assert_eq!(site.operand().displacement, i64::from(displacement));
                assert_eq!(
                    match access {
                        MemorySiteOperationV2::Load => site.destination(),
                        MemorySiteOperationV2::Store => site.source(),
                    },
                    Some(5)
                );
                assert!(site.matches_source_bytes(&bytes, access));
            }

            for bytes in [
                vec![0x48, opcode, 0x6d],          // missing displacement
                vec![0x4c, opcode, 0x75, 0],       // extended data register
                vec![0x48, opcode, 0x7c, 0x25, 0], // RSI data is outside this profile
                vec![0x67, 0x48, opcode, 0x75, 0], // unsupported data register
                vec![0x48, opcode, 0x6d, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x2820, &bytes).unwrap();
                let result = match access {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x2820).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x2820).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_rbp_data_and_base_disp32_is_exact_ad_form_and_distinct_from_rip_relative() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for displacement in [i32::MIN, -1, 0, i32::MAX] {
                let displacement_bytes = displacement.to_le_bytes();
                let source = [
                    0x48,
                    opcode,
                    0xad,
                    displacement_bytes[0],
                    displacement_bytes[1],
                    displacement_bytes[2],
                    displacement_bytes[3],
                ];
                let code = CodeView::new(0x2830, &source).unwrap();
                let (site, copied) = match operation {
                    MemorySiteOperationV2::Load => {
                        let ir = compile_ir_memory_load_block_v2(code, 0x2830).unwrap();
                        let direct = compile_memory_load_block_v2(code, 0x2830).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                    MemorySiteOperationV2::Store => {
                        let ir = compile_ir_memory_store_block_v2(code, 0x2830).unwrap();
                        let direct = compile_memory_store_block_v2(code, 0x2830).unwrap();
                        assert_eq!(ir.site(), direct.site());
                        (ir.site().clone(), ir.source_bytes().to_vec())
                    }
                };
                assert_eq!(copied, source);
                assert_eq!(site.next_rip(), 0x2837);
                assert_eq!(site.operand().base, AddressBase::Register(Register::Rbp));
                assert_eq!(site.operand().index, None);
                assert_eq!(site.operand().scale, 1);
                assert_eq!(site.operand().displacement, i64::from(displacement));
                assert_eq!(site.destination().or(site.source()), Some(5));
                assert!(site.matches_source_bytes(&source, operation));
                let mut rbp_memory = CodeMemory::full_code(code);
                let rbp_decoded =
                    long_decode::decode(&mut rbp_memory, 0x2830, AccessContext::default()).unwrap();
                let rbp_operand = match rbp_decoded.operation {
                    Operation::Mov {
                        dst: Operand::Memory(operand),
                        ..
                    }
                    | Operation::Mov {
                        src: Operand::Memory(operand),
                        ..
                    } => operand,
                    _ => panic!("expected MOV memory operand"),
                };
                let mut address_state = od_isa::LongModeState::default();
                address_state.gpr[5] = 0x3000;
                let rbp_target = rbp_operand
                    .access_address_48(&address_state, 0x2830 + u64::from(rbp_decoded.length))
                    .unwrap();
                assert_eq!(
                    rbp_target,
                    0x3000_u64.wrapping_add_signed(i64::from(displacement))
                );

                let rip_source = [
                    0x48,
                    opcode,
                    0x2d, // same data register, mod00/rm101 means NextRip
                    displacement_bytes[0],
                    displacement_bytes[1],
                    displacement_bytes[2],
                    displacement_bytes[3],
                ];
                let rip_code = CodeView::new(0x2830, &rip_source).unwrap();
                let mut memory = CodeMemory::full_code(rip_code);
                let decoded =
                    long_decode::decode(&mut memory, 0x2830, AccessContext::default()).unwrap();
                let rip_operand = match decoded.operation {
                    Operation::Mov {
                        dst: Operand::Memory(operand),
                        ..
                    }
                    | Operation::Mov {
                        src: Operand::Memory(operand),
                        ..
                    } => operand,
                    _ => panic!("expected MOV memory operand"),
                };
                assert_eq!(rip_operand.base, AddressBase::NextRip);
                assert_eq!(rip_operand.displacement, i64::from(displacement));
                let rip_target = rip_operand
                    .access_address_48(&address_state, 0x2830 + u64::from(decoded.length))
                    .unwrap();
                assert_eq!(
                    rip_target,
                    0x2837_u64.wrapping_add_signed(i64::from(displacement))
                );
                assert_ne!(rbp_target, rip_target);
            }

            for high_byte in [1_u8, 2] {
                let source = [0x48, opcode, 0xad, 0, 0, 0, high_byte];
                let site = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(
                        CodeView::new(0x2830, &source).unwrap(),
                        0x2830,
                    )
                    .unwrap()
                    .site()
                    .clone(),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(
                        CodeView::new(0x2830, &source).unwrap(),
                        0x2830,
                    )
                    .unwrap()
                    .site()
                    .clone(),
                };
                assert_eq!(site.operand().displacement, i64::from(high_byte) << 24);
            }

            for partial_len in 1..7 {
                let source = [0x48, opcode, 0xad, 0, 0, 0, 0];
                let code = CodeView::new(0x2830, &source[..partial_len]).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => compile_ir_memory_load_block_v2(code, 0x2830)
                        .and_then(|_| compile_memory_load_block_v2(code, 0x2830).map(|_| ())),
                    MemorySiteOperationV2::Store => compile_ir_memory_store_block_v2(code, 0x2830)
                        .and_then(|_| compile_memory_store_block_v2(code, 0x2830).map(|_| ())),
                };
                assert!(result.is_err(), "partial len={partial_len}");
            }

            for source in [
                [0x48, opcode, 0xac, 0x24, 0, 0, 0], // SIB
                [0x67, 0x48, opcode, 0xad, 0, 0, 0],
                [0x48, 0x67, opcode, 0xad, 0, 0, 0],
            ] {
                let code = CodeView::new(0x2830, &source).unwrap();
                let result = match operation {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x2830).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x2830).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {source:02x?}");
            }
        }
    }

    #[test]
    fn ir_address32_sib_no_displacement_has_exact_qword_descriptor() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for base in 0..4_u8 {
                    for index in 0..4_u8 {
                        for scale_shift in 0..4_u8 {
                            let bytes = [
                                0x67,
                                0x48,
                                opcode,
                                (data << 3) | 4,
                                (scale_shift << 6) | (index << 3) | base,
                            ];
                            let code = CodeView::new(0x27e0, &bytes).unwrap();
                            let (site, source) = match access {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x27e0).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x27e0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    (ir.site().clone(), ir.source_bytes().to_vec())
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x27e0).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x27e0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    (ir.site().clone(), ir.source_bytes().to_vec())
                                }
                            };
                            assert_eq!(source, bytes);
                            assert_eq!(site.guest_rip(), 0x27e0);
                            assert_eq!(site.next_rip(), 0x27e5);
                            assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand().base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(
                                site.operand().index,
                                Some(Register::from_encoding(index).unwrap())
                            );
                            assert_eq!(site.operand().scale, 1 << scale_shift);
                            assert_eq!(site.operand().displacement, 0);
                            assert!(site.matches_source_bytes(&bytes, access));
                        }
                    }
                }
            }
        }

        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for bytes in [
                vec![0x67, 0x48, opcode],
                vec![0x67, 0x48, opcode, 0x04],       // missing SIB
                vec![0x67, 0x48, opcode, 0xc0, 0x18], // register mode
                vec![0x67, 0x48, opcode, 0x04, 0x60], // no-index with non-unit scale
                vec![0x67, 0x48, opcode, 0x04, 0x25], // no-base and no-index specials
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x04, 0x1d], // no-base SIB with missing disp32
                vec![0x67, 0x4c, opcode, 0x04, 0x18], // extended data
                vec![0x48, 0x67, opcode, 0x04, 0x18], // wrong prefix order
                vec![0x67, 0x48, opcode, 0x04, 0x18, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x27f0, &bytes).unwrap();
                let result = match access {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x27f0).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x27f0).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn ir_address32_sib_disp8_has_exact_qword_descriptor() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for base in 0..4_u8 {
                    for index in 0..4_u8 {
                        for scale_shift in 0..4_u8 {
                            for displacement in [i8::MIN, -1, 0, i8::MAX] {
                                let bytes = [
                                    0x67,
                                    0x48,
                                    opcode,
                                    0x40 | (data << 3) | 4,
                                    (scale_shift << 6) | (index << 3) | base,
                                    displacement as u8,
                                ];
                                let code = CodeView::new(0x2800, &bytes).unwrap();
                                let (site, source) = match access {
                                    MemorySiteOperationV2::Load => {
                                        let ir =
                                            compile_ir_memory_load_block_v2(code, 0x2800).unwrap();
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x2800).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        (ir.site().clone(), ir.source_bytes().to_vec())
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x2800).unwrap();
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x2800).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        (ir.site().clone(), ir.source_bytes().to_vec())
                                    }
                                };
                                assert_eq!(source, bytes);
                                assert_eq!(site.guest_rip(), 0x2800);
                                assert_eq!(site.next_rip(), 0x2806);
                                assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand().base,
                                    AddressBase::Register(Register::from_encoding(base).unwrap())
                                );
                                assert_eq!(
                                    site.operand().index,
                                    Some(Register::from_encoding(index).unwrap())
                                );
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                                assert!(site.matches_source_bytes(&bytes, access));
                            }
                        }
                    }
                }
            }
        }

        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for bytes in [
                vec![0x67, 0x48, opcode],
                vec![0x67, 0x48, opcode, 0x44], // missing SIB and disp8
                vec![0x67, 0x48, opcode, 0x44, 0x18], // missing disp8 after SIB
                vec![0x67, 0x48, opcode, 0x04, 0x18, 0], // wrong ModRM mode
                vec![0x67, 0x48, opcode, 0x44, 0x60, 0], // no-index with non-unit scale
                vec![0x67, 0x48, opcode, 0x74, 0x25, 0], // RSI data is outside this profile
                vec![0x67, 0x48, opcode, 0x74, 0x34, 0], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x44, 0x65, 0], // no-base/no-index, non-unit scale
                vec![0x67, 0x4c, opcode, 0x44, 0x18, 0], // extended data
                vec![0x48, 0x67, opcode, 0x44, 0x18, 0], // wrong prefix order
                vec![0x67, 0x48, opcode, 0x44, 0x18, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x2810, &bytes).unwrap();
                let result = match access {
                    MemorySiteOperationV2::Load => {
                        compile_ir_memory_load_block_v2(code, 0x2810).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_ir_memory_store_block_v2(code, 0x2810).map(|_| ())
                    }
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
        for length in 1..6 {
            let bytes = [0x67, 0x48, 0x8b, 0x44, 0x18, 0x80];
            let code = CodeView::new(0x2820, &bytes[..length]).unwrap();
            assert!(compile_ir_memory_load_block_v2(code, 0x2820).is_err());
        }
    }

    #[test]
    fn ir_address32_sib_disp32_has_exact_nine_byte_qword_descriptor() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..4_u8 {
                for base in 0..4_u8 {
                    for index in 0..4_u8 {
                        for scale_shift in 0..4_u8 {
                            for displacement in [i32::MIN, -1, 0, i32::MAX] {
                                let bytes = [
                                    0x67,
                                    0x48,
                                    opcode,
                                    0x80 | (data << 3) | 4,
                                    (scale_shift << 6) | (index << 3) | base,
                                    displacement.to_le_bytes()[0],
                                    displacement.to_le_bytes()[1],
                                    displacement.to_le_bytes()[2],
                                    displacement.to_le_bytes()[3],
                                ];
                                let code = CodeView::new(0x2830, &bytes).unwrap();
                                let (site, source) = match access {
                                    MemorySiteOperationV2::Load => {
                                        let ir =
                                            compile_ir_memory_load_block_v2(code, 0x2830).unwrap();
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x2830).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        assert_eq!(ir.pc_map().instruction_count(), 1);
                                        assert_eq!(ir.pc_map().boundaries().len(), 1);
                                        assert_eq!(
                                            ir.pc_map().boundaries()[0].rip,
                                            crate::statepoints::GuestRipBoundary::Fixed(0x2839)
                                        );
                                        assert_eq!(direct.source_bytes_storage().1, 9);
                                        (ir.site().clone(), ir.source_bytes().to_vec())
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x2830).unwrap();
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x2830).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        assert_eq!(ir.pc_map().instruction_count(), 1);
                                        assert_eq!(ir.pc_map().boundaries().len(), 1);
                                        assert_eq!(
                                            ir.pc_map().boundaries()[0].rip,
                                            crate::statepoints::GuestRipBoundary::Fixed(0x2839)
                                        );
                                        assert_eq!(direct.source_bytes_storage().1, 9);
                                        (ir.site().clone(), ir.source_bytes().to_vec())
                                    }
                                };
                                assert_eq!(source, bytes);
                                assert_eq!(site.guest_rip(), 0x2830);
                                assert_eq!(site.next_rip(), 0x2839);
                                assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert!(site.matches_source_bytes(&bytes, access));
                            }
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0x84, 0x18], // truncated disp32
                vec![0x67, 0x48, opcode, 0x44, 0x18, 0, 0, 0, 0], // disp8 shape
                vec![0x67, 0x48, opcode, 0x84, 0x60, 0, 0, 0, 0], // no-index with non-unit scale
                vec![0x67, 0x48, opcode, 0xb4, 0x25, 0, 0, 0, 0], // RSI data is outside this profile
                vec![0x67, 0x48, opcode, 0xb4, 0x34, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x84, 0x65, 0, 0, 0, 0], // no-base/no-index, non-unit scale
                vec![0x67, 0x4c, opcode, 0x84, 0x18, 0, 0, 0, 0], // extended data
                vec![0x48, 0x67, opcode, 0x84, 0x18, 0, 0, 0, 0], // wrong prefix order
                vec![0x67, 0x48, opcode, 0x84, 0x18, 0, 0, 0, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x2840, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2840).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2840).map(|_| ())
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
            let bytes = [0x67, 0x48, opcode, 0x84, 0x18, 0, 0, 0, 0];
            for length in 1..9 {
                let code = CodeView::new(0x2850, &bytes[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2850).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2850).map(|_| ())
                };
                assert!(result.is_err(), "partial window length {length} accepted");
            }
        }
    }

    #[test]
    fn address_size_override_signed_displacements_are_admitted_by_decoded_shape() {
        let displacement_cases: &[(u8, i32)] = &[
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, 1),
            (1, i8::MAX as i32),
            (4, i32::MIN),
            (4, i32::MIN + 1),
            (4, -1),
            (4, 0),
            (4, 1),
            (4, i32::MAX - 1),
            (4, i32::MAX),
        ];
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..4_u8 {
                for base in 0..4_u8 {
                    for &(width, displacement) in displacement_cases {
                        let mode = if width == 1 { 1 } else { 2 };
                        let modrm = (mode << 6) | (data_register << 3) | base;
                        let mut bytes = vec![0x67, 0x48, opcode, modrm];
                        if width == 1 {
                            bytes.push(displacement as u8);
                        } else {
                            bytes.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let code = CodeView::new(0x27a0, &bytes).unwrap();
                        let ir_site = match access {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x27a0).unwrap();
                                assert_eq!(
                                    ir.site(),
                                    compile_memory_load_block_v2(code, 0x27a0).unwrap().site()
                                );
                                ir.site().clone()
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x27a0).unwrap();
                                assert_eq!(
                                    ir.site(),
                                    compile_memory_store_block_v2(code, 0x27a0).unwrap().site()
                                );
                                ir.site().clone()
                            }
                        };
                        assert_eq!(ir_site.operand.address_size, od_isa::OperandSize::Dword);
                        assert_eq!(ir_site.operand.size, od_isa::OperandSize::Qword);
                        assert_eq!(ir_site.operand.displacement, i64::from(displacement));
                        assert_eq!(ir_site.next_rip(), 0x27a0 + bytes.len() as u64);
                        assert!(ir_site.matches_source_bytes(&bytes, access));
                        let artifact = match access {
                            MemorySiteOperationV2::Load => {
                                compile_memory_load_block_v2(code, 0x27a0).unwrap().artifact
                            }
                            MemorySiteOperationV2::Store => {
                                compile_memory_store_block_v2(code, 0x27a0)
                                    .unwrap()
                                    .artifact
                            }
                        };
                        assert_eq!(artifact.source_len, bytes.len() as u8);
                        assert_eq!(
                            artifact.site.operand.address_size,
                            od_isa::OperandSize::Dword
                        );
                        assert_eq!(artifact.site.operand.displacement, i64::from(displacement));
                        assert_eq!(artifact.site.next_rip(), 0x27a0 + bytes.len() as u64);
                        assert!(artifact.site.matches_source_bytes(&bytes, access));
                    }
                }
            }
        }

        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0x40],                   // truncated disp8
                vec![0x67, 0x48, opcode, 0x80, 0, 0, 0],          // truncated disp32
                vec![0x67, 0x48, opcode, 0x40, 0, 0x90],          // trailing byte
                vec![0x67, 0x48, opcode, 0x80, 0, 0, 0, 0, 0x90], // trailing byte
                vec![0x67, 0x48, opcode, 0x40 | (4 << 3) | 4, 0], // truncated SIB
                vec![0x67, 0x4c, opcode, 0x40, 0],                // extended data/base register
                vec![0x48, 0x67, opcode, 0x40, 0],                // reordered prefix
            ] {
                let code = CodeView::new(0x27b0, &bytes).unwrap();
                let result = match access {
                    MemorySiteOperationV2::Load => {
                        compile_memory_load_block_v2(code, 0x27b0).map(|_| ())
                    }
                    MemorySiteOperationV2::Store => {
                        compile_memory_store_block_v2(code, 0x27b0).map(|_| ())
                    }
                };
                assert!(
                    matches!(
                        result,
                        Err(MemoryLoadCompileErrorV2::UnsupportedForm
                            | MemoryLoadCompileErrorV2::Decode)
                    ),
                    "unexpectedly admitted bytes {bytes:02x?}"
                );
            }
        }
    }

    #[test]
    fn address32_direct_rsp_rbp_data_is_exact_and_keeps_qword_width() {
        for (opcode, access) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in [4_u8, 5] {
                for base in 0..4_u8 {
                    let cases = [
                        (0_u8, 0_i32),
                        (1, i8::MIN as i32),
                        (1, -1),
                        (1, 0),
                        (1, i8::MAX as i32),
                        (2, i32::MIN),
                        (2, -1),
                        (2, 0),
                        (2, i32::MAX),
                    ];
                    for (mode, displacement) in cases {
                        let mut bytes = vec![
                            0x67,
                            0x48,
                            opcode,
                            (mode << 6) | (data_register << 3) | base,
                        ];
                        match mode {
                            0 => {}
                            1 => bytes.push(displacement as u8),
                            _ => bytes.extend_from_slice(&displacement.to_le_bytes()),
                        }
                        let code = CodeView::new(0x27c0, &bytes).unwrap();
                        let site = match access {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x27c0).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x27c0).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x27c0).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x27c0).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                        };
                        assert!(site.has_supported_profile());
                        assert!(site.matches_source_bytes(&bytes, access));
                        assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                        assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                        assert_eq!(site.operand.displacement, i64::from(displacement));
                        assert_eq!(site.next_rip(), 0x27c0 + bytes.len() as u64);
                        let mut state = od_isa::LongModeState::default();
                        state.gpr[usize::from(base)] = 0xfeed_beef_ffff_fffc;
                        let expected = (0xffff_fffcu32).wrapping_add(displacement as u32) as u64;
                        assert_eq!(
                            site.operand.access_address_48(&state, site.next_rip()),
                            Ok(expected)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn address32_indexed_sib_rsp_rbp_data_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in [4_u8, 5] {
                for (base, index) in [(0_u8, 1_u8), (1, 1), (2, 0), (3, 2), (4, 0), (4, 3)] {
                    for scale_shift in 0..4_u8 {
                        for (mode, displacement) in displacement_cases {
                            let mut bytes = vec![
                                0x67,
                                0x48,
                                opcode,
                                (mode << 6) | (data_register << 3) | 4,
                                (scale_shift << 6) | (index << 3) | base,
                            ];
                            match mode {
                                0 => {}
                                1 => bytes.push(displacement as u8),
                                _ => bytes.extend_from_slice(&displacement.to_le_bytes()),
                            }
                            let code = CodeView::new(0x27d0, &bytes).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x27d0).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x27d0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x27d0).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x27d0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&bytes, operation));
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            assert_eq!(site.operand.scale, 1 << scale_shift);
                            assert_eq!(site.next_rip(), 0x27d0 + bytes.len() as u64);
                        }
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0x24, 0x60], // no-index with non-unit scale
                vec![0x67, 0x48, opcode, 0x24, 0x25], // no-base SIB
                vec![0x67, 0x48, opcode, 0x24, 0x63], // no-index with non-unit scale
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x24, 0x25], // absolute/no-base SIB special form
                vec![0x67, 0x48, opcode, 0x74, 0x35, 0], // data RSI is outside RAX..RBP
                vec![0x67, 0x48, opcode, 0xb4, 0x35, 0, 0, 0, 0], // data RSI is outside RAX..RBP
                vec![0x67, 0x48, opcode, 0x64, 0x08], // missing disp8
                vec![0x67, 0x48, opcode, 0xa4, 0x08, 0, 0, 0], // missing disp32
                vec![0x48, 0x67, opcode, 0x24, 0x08], // reordered prefix
            ] {
                let code = CodeView::new(0x27d0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x27d0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x27d0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid indexed stack-data form admitted {bytes:02x?}"
                );
            }
        }
    }

    #[test]
    fn address32_indexed_sib_rsi_rdi_base_has_exact_qword_profile() {
        let displacements = [
            (0_u8, 0),
            (1_u8, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [6_u8, 7] {
                for index in 0..4_u8 {
                    for scale_shift in 0..4_u8 {
                        for data in 0..6_u8 {
                            for (mode, displacement) in displacements {
                                let mut source = vec![
                                    0x67,
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data << 3) | 4,
                                    (scale_shift << 6) | (index << 3) | base,
                                ];
                                if mode == 1 {
                                    source.push(displacement as i8 as u8);
                                } else if mode == 2 {
                                    source.extend_from_slice(&displacement.to_le_bytes());
                                }
                                let code = CodeView::new(0x2be0, &source).unwrap();
                                let site = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let direct = compile_memory_load_block_v2(code, 0x2be0)
                                            .unwrap_or_else(|error| {
                                                panic!("direct {source:02x?}: {error:?}")
                                            });
                                        let ir = compile_ir_memory_load_block_v2(code, 0x2be0)
                                            .unwrap_or_else(|error| {
                                                panic!("IR {source:02x?}: {error:?}")
                                            });
                                        assert_eq!(direct.site(), ir.site());
                                        ir.site().clone()
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let direct = compile_memory_store_block_v2(code, 0x2be0)
                                            .unwrap_or_else(|error| {
                                                panic!("direct {source:02x?}: {error:?}")
                                            });
                                        let ir = compile_ir_memory_store_block_v2(code, 0x2be0)
                                            .unwrap_or_else(|error| {
                                                panic!("IR {source:02x?}: {error:?}")
                                            });
                                        assert_eq!(direct.site(), ir.site());
                                        ir.site().clone()
                                    }
                                };
                                assert!(site.has_supported_profile());
                                assert!(site.matches_source_bytes(&source, operation));
                                assert_eq!(site.next_rip(), 0x2be0 + source.len() as u64);
                                assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand.base,
                                    AddressBase::Register(Register::from_encoding(base).unwrap())
                                );
                                assert_eq!(
                                    site.operand.index,
                                    Some(Register::from_encoding(index).unwrap())
                                );
                                assert_eq!(site.operand.scale, 1 << scale_shift);
                                assert_eq!(site.operand.displacement, i64::from(displacement));
                            }
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x04, 0x65], // no-index scale neighbor
                vec![0x67, 0x48, opcode, 0x34, 0x36], // RSI data register
                vec![0x67, 0x48, opcode, 0x34, 0x3e], // RSI data register
                vec![0x48, 0x67, opcode, 0x04, 0x06], // reordered prefix
            ] {
                let code = CodeView::new(0x2be0, &source).unwrap();
                let rejected = match opcode {
                    0x8b => {
                        compile_ir_memory_load_block_v2(code, 0x2be0).is_err()
                            && compile_memory_load_block_v2(code, 0x2be0).is_err()
                    }
                    _ => {
                        compile_ir_memory_store_block_v2(code, 0x2be0).is_err()
                            && compile_memory_store_block_v2(code, 0x2be0).is_err()
                    }
                };
                assert!(
                    rejected,
                    "unsupported neighboring form admitted {source:02x?}"
                );
            }
            let disp8 = [0x67, 0x48, opcode, 0x44, 0x06, 0x80];
            for window in 1..disp8.len() {
                let code = CodeView::new(0x2be0, &disp8[..window]).unwrap();
                let rejected = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2be0).is_err()
                        && compile_memory_load_block_v2(code, 0x2be0).is_err()
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2be0).is_err()
                        && compile_memory_store_block_v2(code, 0x2be0).is_err()
                };
                assert!(rejected, "partial disp8 window admitted {window}");
            }
            let disp32 = [0x67, 0x48, opcode, 0x84, 0x06, 0, 0, 0, 1];
            for window in 1..disp32.len() {
                let code = CodeView::new(0x2be0, &disp32[..window]).unwrap();
                let rejected = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2be0).is_err()
                        && compile_memory_load_block_v2(code, 0x2be0).is_err()
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2be0).is_err()
                        && compile_memory_store_block_v2(code, 0x2be0).is_err()
                };
                assert!(rejected, "partial disp32 window admitted {window}");
            }
        }
    }

    #[test]
    fn address32_sib_rsp_base_rbp_index_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..6 {
                for scale_shift in 0..4_u8 {
                    for &(mode, displacement) in &displacement_cases {
                        let mut bytes = vec![
                            0x67,
                            0x48,
                            opcode,
                            (mode << 6) | (data_register << 3) | 4,
                            (scale_shift << 6) | (5 << 3) | 4,
                        ];
                        match mode {
                            0 => {}
                            1 => bytes.push(displacement as u8),
                            _ => bytes.extend_from_slice(&displacement.to_le_bytes()),
                        }
                        let code = CodeView::new(0x29a0, &bytes).unwrap();
                        let site = match operation {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x29a0).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x29a0).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x29a0).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x29a0).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                        };
                        assert!(site.has_supported_profile());
                        assert!(site.matches_source_bytes(&bytes, operation));
                        assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                        assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                        assert_eq!(
                            site.operand.base,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                        );
                        assert_eq!(site.operand.index, Some(od_isa::Register::Rbp));
                        assert_eq!(site.operand.scale, 1 << scale_shift);
                        assert_eq!(site.operand.displacement, i64::from(displacement));
                        assert_eq!(site.next_rip(), 0x29a0 + bytes.len() as u64);
                        let mut state = od_isa::LongModeState::default();
                        state.gpr[4] = 0xface_cafe_ffff_fffdu64;
                        state.gpr[5] = 0xdead_beef_8000_0003u64;
                        let expected = (state.gpr[4] as u32)
                            .wrapping_add((state.gpr[5] as u32).wrapping_mul(1_u32 << scale_shift))
                            .wrapping_add(displacement as u32)
                            as u64;
                        assert_eq!(
                            site.operand.access_address_48(&state, site.next_rip()),
                            Ok(expected)
                        );
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0x34, 0x2c], // unsupported data register
                vec![0x67, 0x48, opcode, 0x34, 0x34], // data RSI
                vec![0x67, 0x48, opcode, 0x74, 0x36, 0], // RSI data is outside the whitelist
                vec![0x67, 0x48, opcode, 0x04, 0x2d], // no base
                vec![0x67, 0x48, opcode, 0x64, 0x6c], // missing disp8
                vec![0x67, 0x48, opcode, 0xa4, 0xac, 0, 0, 0], // missing disp32
                vec![0x48, 0x67, opcode, 0x04, 0x2c], // reordered prefix
            ] {
                let code = CodeView::new(0x29a0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x29a0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x29a0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid RSP-base/RBP-index profile admitted {bytes:02x?}"
                );
            }
        }
    }

    #[test]
    fn address32_sib_rbp_index_has_exact_qword_profile() {
        let displacements = [
            (0_u8, vec![0_i32]),
            (1, vec![i8::MIN as i32, -1, 0, i8::MAX as i32]),
            (2, vec![i32::MIN, -1, 0, 1, i32::MAX]),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for base in 0_u8..4 {
                    for scale_shift in 0..4_u8 {
                        for (mode, cases) in &displacements {
                            for displacement in cases {
                                let mut bytes = vec![
                                    0x67,
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data_register << 3) | 4,
                                    (scale_shift << 6) | (5 << 3) | base,
                                ];
                                match mode {
                                    0 => {}
                                    1 => bytes.push(*displacement as u8),
                                    _ => bytes.extend_from_slice(&displacement.to_le_bytes()),
                                }
                                let code = CodeView::new(0x29a0, &bytes).unwrap();
                                let mut memory = CodeMemory::full_code(code);
                                let decoded = od_isa::long_decode::decode(
                                    &mut memory,
                                    0x29a0,
                                    AccessContext::default(),
                                )
                                .unwrap();
                                let admitted = match operation {
                                    MemorySiteOperationV2::Load => {
                                        crate::block::is_checked_qword_load_form(decoded)
                                    }
                                    MemorySiteOperationV2::Store => {
                                        crate::block::is_checked_qword_store_form(decoded)
                                    }
                                };
                                assert!(
                                    admitted,
                                    "decoded form rejected: {bytes:02x?} {decoded:?}"
                                );
                                assert!(
                                    crate::block::exact_qword_address32_rbp_index_sib_memory_bytes(
                                        code,
                                        0x29a0,
                                        decoded.length,
                                        opcode
                                    ),
                                    "source bytes rejected: {bytes:02x?} {decoded:?}"
                                );
                                let operand = match decoded.operation {
                                    Operation::Mov {
                                        dst: Operand::Register(_),
                                        src: Operand::Memory(operand),
                                    }
                                    | Operation::Mov {
                                        dst: Operand::Memory(operand),
                                        src: Operand::Register(_),
                                    } => operand,
                                    _ => panic!("not a MOV: {decoded:?}"),
                                };
                                let probe_site = MemorySiteV2 {
                                    instruction_index: 0,
                                    guest_rip: 0x29a0,
                                    next_rip: 0x29a0 + u64::from(decoded.length),
                                    operation,
                                    destination: (operation == MemorySiteOperationV2::Load)
                                        .then_some(data_register),
                                    source: (operation == MemorySiteOperationV2::Store)
                                        .then_some(data_register),
                                    operand,
                                };
                                assert!(
                                    probe_site.has_supported_profile(),
                                    "runtime profile rejected: {bytes:02x?} {probe_site:?}"
                                );
                                assert!(
                                    probe_site.matches_source_bytes(&bytes, operation),
                                    "runtime matcher rejected: {bytes:02x?} {probe_site:?}"
                                );
                                let site = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let ir = compile_ir_memory_load_block_v2(code, 0x29a0)
                                            .unwrap_or_else(|error| {
                                                panic!("{bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x29a0).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        ir.site().clone()
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir = compile_ir_memory_store_block_v2(code, 0x29a0)
                                            .unwrap_or_else(|error| {
                                                panic!("{bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x29a0).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        ir.site().clone()
                                    }
                                };
                                assert!(site.has_supported_profile());
                                assert!(site.matches_source_bytes(&bytes, operation));
                                assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand.base,
                                    od_isa::long_decode::AddressBase::Register(
                                        od_isa::Register::from_encoding(base).unwrap()
                                    )
                                );
                                assert_eq!(site.operand.index, Some(od_isa::Register::Rbp));
                                assert_eq!(site.operand.scale, 1 << scale_shift);
                                assert_eq!(site.operand.displacement, i64::from(*displacement));
                                assert_eq!(site.next_rip(), 0x29a0 + bytes.len() as u64);
                            }
                        }
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for (modrm, sib, suffix) in [
                (0x04, 0x65, vec![]),           // mod00 no-base with invalid no-index scale
                (0x34, 0x34, vec![]),           // unsupported RSI data register
                (0x74, 0x36, vec![0]),          // RSI data is outside the whitelist
                (0xb4, 0x36, vec![0, 0, 0, 0]), // RSI data is outside the whitelist
                (0x74, 0x34, vec![0]),          // unsupported RSI data register
            ] {
                let mut bytes = vec![0x67, 0x48, opcode, modrm, sib];
                bytes.extend_from_slice(&suffix);
                let code = CodeView::new(0x29a0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x29a0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x29a0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid RBP-index profile admitted {bytes:02x?}"
                );
            }
        }
        let low_byte_source = [0x67, 0x48, 0x8b, 0x84, 0x28, 0, 0, 0, 0];
        let high_byte_source = [0x67, 0x48, 0x8b, 0x84, 0x28, 0, 0, 0, 1];
        assert_eq!(&low_byte_source[..8], &high_byte_source[..8]);
        assert_ne!(low_byte_source[8], high_byte_source[8]);
        let low_site = compile_ir_memory_load_block_v2(
            CodeView::new(0x29a0, &low_byte_source).unwrap(),
            0x29a0,
        )
        .unwrap()
        .site()
        .clone();
        let high_site = compile_ir_memory_load_block_v2(
            CodeView::new(0x29a0, &high_byte_source).unwrap(),
            0x29a0,
        )
        .unwrap()
        .site()
        .clone();
        let mut state = od_isa::LongModeState::default();
        state.gpr[0] = 0x100;
        state.gpr[5] = 2;
        assert_eq!(low_site.operand.displacement, 0);
        assert_eq!(high_site.operand.displacement, 0x0100_0000);
        assert_eq!(
            low_site
                .operand
                .access_address_48(&state, low_site.next_rip()),
            Ok(0x102)
        );
        assert_eq!(
            high_site
                .operand
                .access_address_48(&state, high_site.next_rip()),
            Ok(0x0100_0102)
        );
    }

    #[test]
    fn address32_sib_rbp_base_rbp_index_has_exact_qword_profile() {
        let displacement_cases = [
            (1_u8, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for scale_shift in 0..4_u8 {
                    for &(mode, displacement) in &displacement_cases {
                        let mut bytes = vec![
                            0x67,
                            0x48,
                            opcode,
                            (mode << 6) | (data_register << 3) | 4,
                            (scale_shift << 6) | (5 << 3) | 5,
                        ];
                        if mode == 1 {
                            bytes.push(displacement as u8);
                        } else {
                            bytes.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let code = CodeView::new(0x29a0, &bytes).unwrap();
                        let site = match operation {
                            MemorySiteOperationV2::Load => {
                                let ir = compile_ir_memory_load_block_v2(code, 0x29a0).unwrap();
                                let direct = compile_memory_load_block_v2(code, 0x29a0).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                            MemorySiteOperationV2::Store => {
                                let ir = compile_ir_memory_store_block_v2(code, 0x29a0).unwrap();
                                let direct = compile_memory_store_block_v2(code, 0x29a0).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                        };
                        assert!(site.has_supported_profile());
                        assert!(site.matches_source_bytes(&bytes, operation));
                        assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                        assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                        assert_eq!(
                            site.operand.base,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                        );
                        assert_eq!(site.operand.index, Some(od_isa::Register::Rbp));
                        assert_eq!(site.operand.scale, 1 << scale_shift);
                        assert_eq!(site.operand.displacement, i64::from(displacement));
                        assert_eq!(site.next_rip(), 0x29a0 + bytes.len() as u64);
                        let mut state = od_isa::LongModeState::default();
                        state.gpr[5] = 0xdead_beef_ffff_fffd;
                        let low_rbp = state.gpr[5] as u32;
                        let expected = low_rbp
                            .wrapping_add(low_rbp.wrapping_mul(1_u32 << scale_shift))
                            .wrapping_add(displacement as u32)
                            as u64;
                        assert_eq!(
                            site.operand.access_address_48(&state, site.next_rip()),
                            Ok(expected)
                        );
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for (mode, data, index, suffix) in [
                (0_u8, 0_u8, 5_u8, vec![]),  // mod00 base=RBP is no-base
                (0, 0, 4, vec![0, 0, 0, 0]), // no-index non-unit scale is unsupported
                (1, 6, 6, vec![0]),          // data RSI is outside RAX..RBP
            ] {
                let mut bytes = vec![
                    0x67,
                    0x48,
                    opcode,
                    (mode << 6) | (data << 3) | 4,
                    (index << 3) | 5 | if index == 4 { 0x40 } else { 0 },
                ];
                bytes.extend_from_slice(&suffix);
                if index == 4 {
                    assert_eq!(
                        bytes[4], 0x65,
                        "expected non-unit scale/no-index: {bytes:02x?}"
                    );
                }
                let code = CodeView::new(0x29a0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x29a0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x29a0).map(|_| ())
                };
                assert!(result.is_err(), "invalid profile admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn address32_indexed_sib_rbp_base_disp_has_exact_qword_profile() {
        let displacement_cases = [
            (1_u8, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for index in 0..4_u8 {
                    for scale_shift in 0..4_u8 {
                        for &(mode, displacement) in &displacement_cases {
                            let mut bytes = vec![
                                0x67,
                                0x48,
                                opcode,
                                (mode << 6) | (data_register << 3) | 4,
                                (scale_shift << 6) | (index << 3) | 5,
                            ];
                            if mode == 1 {
                                bytes.push(displacement as u8);
                            } else {
                                bytes.extend_from_slice(&displacement.to_le_bytes());
                            }
                            let code = CodeView::new(0x27d0, &bytes).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x27d0).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x27d0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x27d0).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x27d0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&bytes, operation));
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand.base,
                                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                            );
                            assert_eq!(
                                site.operand.index,
                                Some(od_isa::Register::from_encoding(index).unwrap())
                            );
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            assert_eq!(site.operand.scale, 1 << scale_shift);
                            assert_eq!(site.next_rip(), 0x27d0 + bytes.len() as u64);
                        }
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x67, 0x48, opcode, 0x34, 0x36, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x74, 0x25, 0],          // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x74, 0x36, 0],          // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x44, 0x0d],             // missing disp8
                vec![0x67, 0x48, opcode, 0x84, 0x0d, 0, 0, 0],    // missing disp32
                vec![0x48, 0x67, opcode, 0x44, 0x0d, 0],          // reordered prefix
            ] {
                let code = CodeView::new(0x27d0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x27d0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x27d0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid RBP-indexed SIB admitted {bytes:02x?}"
                );
            }
        }
    }

    #[test]
    fn absolute_sib_disp32_has_exact_qword_profile_for_all_data_registers() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for displacement in [i32::MIN, -8, -1, 0, i32::MAX] {
                    let mut source = vec![0x48, opcode, (data_register << 3) | 4, 0x25];
                    source.extend_from_slice(&displacement.to_le_bytes());
                    let code = CodeView::new(0x2a00, &source).unwrap();
                    let site = match operation {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x2a00)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            let direct = compile_memory_load_block_v2(code, 0x2a00).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            ir.site().clone()
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x2a00).unwrap();
                            let direct = compile_memory_store_block_v2(code, 0x2a00).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            ir.site().clone()
                        }
                    };
                    assert!(site.has_supported_profile());
                    assert!(site.matches_source_bytes(&source, operation));
                    assert_eq!(site.operand.base, od_isa::long_decode::AddressBase::None);
                    assert_eq!(site.operand.index, None);
                    assert_eq!(site.operand.scale, 1);
                    assert_eq!(site.operand.address_size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand.displacement, i64::from(displacement));
                    assert_eq!(site.next_rip(), 0x2a08);
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // invalid no-index scale
                vec![0x48, opcode, 0x74, 0x25, 0],          // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x34, 0x36, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x48, opcode, 0x04, 0x25, 0, 0, 0],    // truncated disp32
                vec![0x48, opcode, 0x04, 0x24, 0, 0, 0, 0], // RSP no-index + tail
                vec![0x48, opcode, 0x04, 0x25, 0, 0, 0, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x2a00, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2a00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2a00).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid absolute SIB form admitted {source:02x?}"
                );
            }
            let complete = [0x48, opcode, 0x04, 0x25, 0, 0, 0, 0];
            for length in 1..8 {
                let code = CodeView::new(0x2a00, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2a00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2a00).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "partial absolute SIB admitted {length} bytes"
                );
            }
        }
    }

    #[test]
    fn address32_absolute_sib_disp32_has_exact_qword_profile_for_all_data_registers() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for displacement in [i32::MIN, -8, -1, 0, i32::MAX] {
                    let mut source = vec![0x67, 0x48, opcode, (data_register << 3) | 4, 0x25];
                    source.extend_from_slice(&displacement.to_le_bytes());
                    let code = CodeView::new(0x2a80, &source).unwrap();
                    let site = match operation {
                        MemorySiteOperationV2::Load => {
                            let direct = compile_memory_load_block_v2(code, 0x2a80)
                                .unwrap_or_else(|error| panic!("direct {source:02x?}: {error:?}"));
                            let ir = compile_ir_memory_load_block_v2(code, 0x2a80)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            assert_eq!(ir.site(), direct.site());
                            ir.site().clone()
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x2a80)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            let direct = compile_memory_store_block_v2(code, 0x2a80).unwrap();
                            assert_eq!(ir.site(), direct.site());
                            ir.site().clone()
                        }
                    };
                    assert!(site.has_supported_profile());
                    assert!(site.matches_source_bytes(&source, operation));
                    assert_eq!(site.operand.base, od_isa::long_decode::AddressBase::None);
                    assert_eq!(site.operand.index, None);
                    assert_eq!(site.operand.scale, 1);
                    assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                    assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                    assert_eq!(site.operand.displacement, i64::from(displacement));
                    assert_eq!(site.next_rip(), 0x2a89);
                    assert!(site.matches_source_bytes(&source, operation));
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x34, 0x36, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x05, 0, 0, 0, 0],       // direct rm=101 neighbor
                vec![0x67, 0x48, opcode, 0x04, 0x24, 0, 0, 0, 0], // RSP no-index with tail
                vec![0x48, 0x67, opcode, 0x04, 0x25, 0, 0, 0, 0], // wrong prefix order
                vec![0x67, 0x48, opcode, 0x04, 0x25, 0, 0, 0, 0, 0x90], // trailing byte
            ] {
                let code = CodeView::new(0x2a80, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2a80).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2a80).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid address32 absolute SIB: {source:02x?}"
                );
            }
            let complete = [0x67, 0x48, opcode, 0x04, 0x25, 0, 0, 0, 0];
            for length in 1..9 {
                let code = CodeView::new(0x2a80, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2a80).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2a80).map(|_| ())
                };
                assert!(result.is_err(), "partial address32 absolute SIB: {length}");
            }
        }
    }

    #[test]
    fn address32_rsp_sib_base_has_exact_qword_profile_for_all_data_registers() {
        let cases = [
            (0_u8, 5_usize, 0_i32),
            (1, 6, i8::MIN as i32),
            (1, 6, -1),
            (1, 6, 0),
            (1, 6, i8::MAX as i32),
            (2, 9, i32::MIN),
            (2, 9, -1),
            (2, 9, 0),
            (2, 9, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for &(mode, expected_len, displacement) in &cases {
                    let modrm = (mode << 6) | (data_register << 3) | 4;
                    let mut source = vec![0x67, 0x48, opcode, modrm, 0x24];
                    match mode {
                        0 => {}
                        1 => source.push(displacement as u8),
                        _ => source.extend_from_slice(&displacement.to_le_bytes()),
                    }
                    assert_eq!(source.len(), expected_len);
                    let code = CodeView::new(0x2a40, &source).unwrap();
                    let (ir_site, direct_site, ir_source) = match operation {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x2a40)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            let direct = compile_memory_load_block_v2(code, 0x2a40)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x2a40)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            let direct = compile_memory_store_block_v2(code, 0x2a40)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                    };
                    assert_eq!(ir_site, direct_site);
                    assert_eq!(ir_source, source);
                    assert!(ir_site.has_supported_profile());
                    assert!(ir_site.matches_source_bytes(&source, operation));
                    assert_eq!(ir_site.next_rip(), 0x2a40 + expected_len as u64);
                    assert_eq!(ir_site.operand().address_size, od_isa::OperandSize::Dword);
                    assert_eq!(ir_site.operand().size, od_isa::OperandSize::Qword);
                    assert_eq!(ir_site.operand().base, AddressBase::Register(Register::Rsp));
                    assert_eq!(ir_site.operand().index, None);
                    assert_eq!(ir_site.operand().scale, 1);
                    assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x04],                // truncated before SIB
                vec![0x67, 0x48, opcode, 0x04, 0x25],          // no-base SIB
                vec![0x67, 0x48, opcode, 0x34, 0x34],          // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x04, 0x64],          // scale 2, no index
                vec![0x67, 0x48, opcode, 0x74, 0x25, 0],       // RSI data is outside this profile
                vec![0x67, 0x48, opcode, 0x84, 0x24, 0, 0, 0], // short disp32
                vec![0x48, 0x67, opcode, 0x04, 0x24],          // reordered prefix
                vec![0x67, 0x48, opcode, 0x34, 0x24],          // data register 6
            ] {
                let code = CodeView::new(0x2a40, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2a40).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2a40).map(|_| ())
                };
                assert!(result.is_err(), "unexpectedly admitted {source:02x?}");
            }
        }
    }

    #[test]
    fn address32_rbp_direct_base_has_exact_qword_profile() {
        let cases = [
            (1_u8, 5_usize, i8::MIN as i32),
            (1, 5, -1),
            (1, 5, 0),
            (1, 5, i8::MAX as i32),
            (2, 8, i32::MIN),
            (2, 8, -1),
            (2, 8, 0),
            (2, 8, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for &(mode, expected_len, displacement) in &cases {
                    let modrm = (mode << 6) | (data_register << 3) | 5;
                    let mut source = vec![0x67, 0x48, opcode, modrm];
                    if mode == 1 {
                        source.push(displacement as u8);
                    } else {
                        source.extend_from_slice(&displacement.to_le_bytes());
                    }
                    assert_eq!(source.len(), expected_len);
                    let code = CodeView::new(0x2b20, &source).unwrap();
                    let (ir_site, direct_site, ir_source) = match operation {
                        MemorySiteOperationV2::Load => {
                            let ir = compile_ir_memory_load_block_v2(code, 0x2b20)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            let direct = compile_memory_load_block_v2(code, 0x2b20)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                        MemorySiteOperationV2::Store => {
                            let ir = compile_ir_memory_store_block_v2(code, 0x2b20)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            let direct = compile_memory_store_block_v2(code, 0x2b20)
                                .unwrap_or_else(|error| panic!("{source:02x?}: {error:?}"));
                            (
                                ir.site().clone(),
                                direct.site().clone(),
                                ir.source_bytes().to_vec(),
                            )
                        }
                    };
                    assert_eq!(ir_site, direct_site);
                    assert_eq!(ir_source, source);
                    assert!(ir_site.has_supported_profile());
                    assert!(ir_site.matches_source_bytes(&source, operation));
                    assert_eq!(ir_site.operand().address_size, od_isa::OperandSize::Dword);
                    assert_eq!(ir_site.operand().size, od_isa::OperandSize::Qword);
                    assert_eq!(ir_site.operand().base, AddressBase::Register(Register::Rbp));
                    assert_eq!(ir_site.operand().index, None);
                    assert_eq!(ir_site.operand().scale, 1);
                    assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                    assert_eq!(ir_site.next_rip(), 0x2b20 + expected_len as u64);
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            let complete = [
                vec![0x67, 0x48, opcode, 0x4d, 0],
                vec![0x67, 0x48, opcode, 0x8d, 0, 0, 0, 0],
            ];
            for source in &complete {
                for partial in 1..source.len() {
                    let code = CodeView::new(0x2b20, &source[..partial]).unwrap();
                    let result = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(code, 0x2b20).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(code, 0x2b20).map(|_| ())
                    };
                    assert!(result.is_err(), "admitted partial {partial}: {source:02x?}");
                }
            }
            for invalid in [
                vec![0x67, 0x48, opcode, 0x05, 0, 0, 0, 0], // mod00/rm101 absolute/RIP-like
                vec![0x67, 0x48, opcode, 0x7c, 0x34, 0],    // unsupported RSI data register
                vec![0x64, 0x67, 0x48, opcode, 0x4d, 0],    // extra segment prefix
                vec![0x48, 0x67, opcode, 0x4d, 0],          // reordered prefixes
                vec![0x67, 0x48, opcode, 0x75, 0],          // unsupported data register
                vec![0x67, 0x48, opcode, 0x4d, 0, 0x90],    // trailing byte
            ] {
                let code = CodeView::new(0x2b20, &invalid).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2b20).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2b20).map(|_| ())
                };
                assert!(result.is_err(), "admitted invalid {invalid:02x?}");
            }
        }
    }

    #[test]
    fn admits_all_valid_sib_register_scale_combinations_for_load_and_store() {
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for data_register in 0..4_u8 {
                for base in 0..4_u8 {
                    for index in 0..4_u8 {
                        for scale_shift in 0..4_u8 {
                            let modrm = 0x04 | (data_register << 3);
                            let sib = (scale_shift << 6) | (index << 3) | base;
                            let bytes = [0x48, opcode, modrm, sib];
                            let code = CodeView::new(0x2800, &bytes).unwrap();
                            let block = if is_load {
                                let block = compile_memory_load_block_v2(code, 0x2800).unwrap();
                                assert_eq!(block.source_bytes(), &bytes);
                                assert_eq!(block.site().destination(), Some(data_register));
                                block.artifact
                            } else {
                                let block = compile_memory_store_block_v2(code, 0x2800).unwrap();
                                assert_eq!(block.source_bytes(), &bytes);
                                assert_eq!(block.site().source(), Some(data_register));
                                block.artifact
                            };
                            assert_eq!(block.source_len, 4);
                            assert_eq!(
                                block.site.operand.base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(block.site.operand.index, Register::from_encoding(index));
                            assert_eq!(block.site.operand.scale, 1 << scale_shift);
                            assert_eq!(block.site.operand.displacement, 0);
                            assert_eq!(block.site.next_rip(), 0x2804);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn admits_supported_direct_and_sib_displacement_shapes_for_both_operations() {
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for mode in 0..=2_u8 {
                for data_register in 0..4_u8 {
                    for base in 0..4_u8 {
                        for addressing in 0..=1_u8 {
                            let mut bytes = vec![0x48, opcode];
                            let rm = if addressing == 0 { base } else { 4 };
                            bytes.push((mode << 6) | (data_register << 3) | rm);
                            if addressing == 1 {
                                bytes.push((2 << 6) | (1 << 3) | base); // scale 4, index RCX
                            }
                            let displacement = match mode {
                                0 => 0_i32,
                                1 => -1,
                                2 => 1,
                                _ => unreachable!(),
                            };
                            match mode {
                                0 => {}
                                1 => bytes.push(displacement as i8 as u8),
                                2 => bytes.extend_from_slice(&displacement.to_le_bytes()),
                                _ => unreachable!(),
                            }
                            let code = CodeView::new(0x5100, &bytes).unwrap();
                            let artifact = if is_load {
                                compile_memory_load_block_v2(code, 0x5100).unwrap().artifact
                            } else {
                                compile_memory_store_block_v2(code, 0x5100)
                                    .unwrap()
                                    .artifact
                            };
                            assert_eq!(
                                &artifact.source_bytes[..artifact.source_len as usize],
                                bytes
                            );
                            assert_eq!(artifact.source_len as usize, bytes.len());
                            assert_eq!(artifact.site.operand.displacement, i64::from(displacement));
                            assert_eq!(artifact.site.next_rip(), 0x5100 + bytes.len() as u64);
                            assert!(artifact.site.matches_source_bytes(
                                &artifact.source_bytes[..artifact.source_len as usize],
                                if is_load {
                                    MemorySiteOperationV2::Load
                                } else {
                                    MemorySiteOperationV2::Store
                                }
                            ));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn signed_displacement_boundaries_and_equivalent_widths_use_decoder_metadata() {
        for is_load in [true, false] {
            for use_sib in [false, true] {
                for displacement in [-128_i32, -1, 0, 1, 127] {
                    let bytes = displacement_form(is_load, use_sib, 1, displacement);
                    let code = CodeView::new(0x5200, &bytes).unwrap();
                    let artifact = if is_load {
                        compile_memory_load_block_v2(code, 0x5200).unwrap().artifact
                    } else {
                        compile_memory_store_block_v2(code, 0x5200)
                            .unwrap()
                            .artifact
                    };
                    assert_eq!(artifact.site.operand.displacement, i64::from(displacement));
                    assert_eq!(
                        &artifact.source_bytes[..artifact.source_len as usize],
                        bytes
                    );
                }
                for displacement in [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX] {
                    let bytes = displacement_form(is_load, use_sib, 2, displacement);
                    let code = CodeView::new(0x5300, &bytes).unwrap();
                    let artifact = if is_load {
                        compile_memory_load_block_v2(code, 0x5300).unwrap().artifact
                    } else {
                        compile_memory_store_block_v2(code, 0x5300)
                            .unwrap()
                            .artifact
                    };
                    assert_eq!(artifact.site.operand.displacement, i64::from(displacement));
                    assert_eq!(
                        &artifact.source_bytes[..artifact.source_len as usize],
                        bytes
                    );
                }

                let disp8 = displacement_form(is_load, use_sib, 1, -1);
                let disp32 = displacement_form(is_load, use_sib, 2, -1);
                let disp8_code = CodeView::new(0x5400, &disp8).unwrap();
                let disp32_code = CodeView::new(0x5400, &disp32).unwrap();
                let disp8_artifact = if is_load {
                    compile_memory_load_block_v2(disp8_code, 0x5400)
                        .unwrap()
                        .artifact
                } else {
                    compile_memory_store_block_v2(disp8_code, 0x5400)
                        .unwrap()
                        .artifact
                };
                let disp32_artifact = if is_load {
                    compile_memory_load_block_v2(disp32_code, 0x5400)
                        .unwrap()
                        .artifact
                } else {
                    compile_memory_store_block_v2(disp32_code, 0x5400)
                        .unwrap()
                        .artifact
                };
                let state = od_isa::LongModeState {
                    gpr: [100, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    ..od_isa::LongModeState::default()
                };
                assert_eq!(
                    disp8_artifact
                        .site
                        .operand
                        .effective_address(&state, disp8_artifact.site.next_rip()),
                    disp32_artifact
                        .site
                        .operand
                        .effective_address(&state, disp32_artifact.site.next_rip())
                );
                assert_ne!(
                    disp8_artifact.site.next_rip(),
                    disp32_artifact.site.next_rip()
                );
            }
        }
    }

    #[test]
    fn admits_only_exact_rip_relative_mod00_disp32_sites() {
        for (opcode, is_load) in [(0x8b, true), (0x89, false)] {
            for register in [0_u8, 1, 2, 3, 6, 7] {
                for displacement in [i32::MIN, -8, -7, -1, 0, 1, 8, i32::MAX] {
                    let mut instruction = vec![0x48, opcode, (register << 3) | 5];
                    instruction.extend_from_slice(&displacement.to_le_bytes());
                    let mut view_bytes = vec![0x90, 0x90];
                    view_bytes.extend_from_slice(&instruction);
                    let code = CodeView::new(0x6000, &view_bytes).unwrap();
                    let entry_rip = 0x6002;
                    let (artifact, ir_site) = if is_load {
                        let direct = compile_memory_load_block_v2(code, entry_rip)
                            .unwrap()
                            .artifact;
                        let ir = compile_ir_memory_load_block_v2(code, entry_rip).unwrap_or_else(
                            |error| panic!("IR load rejected {instruction:02x?}: {error:?}"),
                        );
                        (direct, ir.site().clone())
                    } else {
                        let direct = compile_memory_store_block_v2(code, entry_rip)
                            .unwrap()
                            .artifact;
                        let ir = compile_ir_memory_store_block_v2(code, entry_rip).unwrap_or_else(
                            |error| {
                                panic!(
                                    "IR store rejected {instruction:02x?}: {error:?}, block: {:?}",
                                    crate::block_compile::compile_checked_store_block_v2(
                                        code, entry_rip
                                    )
                                )
                            },
                        );
                        (direct, ir.site().clone())
                    };
                    assert_eq!(artifact.source_len, 7);
                    assert_eq!(&artifact.source_bytes[..7], instruction.as_slice());
                    assert_eq!(artifact.site.guest_rip(), entry_rip);
                    assert_eq!(artifact.site.next_rip(), entry_rip + 7);
                    assert_eq!(artifact.site.operand.base, AddressBase::NextRip);
                    assert_eq!(artifact.site.operand.index, None);
                    assert_eq!(artifact.site.operand.scale, 1);
                    assert_eq!(artifact.site.operand.displacement, i64::from(displacement));
                    assert_eq!(artifact.site, ir_site);
                    assert!(artifact.site.has_supported_profile());
                    assert!(artifact.site.matches_source_bytes(
                        &artifact.source_bytes[..7],
                        if is_load {
                            MemorySiteOperationV2::Load
                        } else {
                            MemorySiteOperationV2::Store
                        }
                    ));
                }
            }
        }
    }

    #[test]
    fn rip_relative_rsi_rdi_sites_reject_forged_descriptors_and_partial_windows() {
        for (opcode, operation) in [
            (0x8b_u8, MemorySiteOperationV2::Load),
            (0x89_u8, MemorySiteOperationV2::Store),
        ] {
            for data_register in [6_u8, 7] {
                let mut source = vec![0x48, opcode, (data_register << 3) | 5];
                source.extend_from_slice(&(-17_i32).to_le_bytes());
                let code = CodeView::new(0x6800, &source).unwrap();
                let direct_site = if operation == MemorySiteOperationV2::Load {
                    compile_memory_load_block_v2(code, 0x6800)
                        .unwrap()
                        .site()
                        .clone()
                } else {
                    compile_memory_store_block_v2(code, 0x6800)
                        .unwrap()
                        .site()
                        .clone()
                };
                let ir_site = if operation == MemorySiteOperationV2::Load {
                    compile_ir_memory_load_block_v2(code, 0x6800)
                        .unwrap()
                        .site()
                        .clone()
                } else {
                    compile_ir_memory_store_block_v2(code, 0x6800)
                        .unwrap()
                        .site()
                        .clone()
                };
                assert_eq!(direct_site, ir_site);
                assert!(direct_site.matches_source_bytes(&source, operation));
                let mut changed_final_byte = source.clone();
                changed_final_byte[6] ^= 1;
                assert!(!direct_site.matches_source_bytes(&changed_final_byte, operation));
                let mut changed_data = source.clone();
                changed_data[2] = ((if data_register == 6 { 7 } else { 6 }) << 3) | 5;
                assert!(!direct_site.matches_source_bytes(&changed_data, operation));
                let mut changed_base = source.clone();
                changed_base[2] = (data_register << 3) | 4;
                assert!(!direct_site.matches_source_bytes(&changed_base, operation));
                assert!(!direct_site.matches_source_bytes(&source[..6], operation));

                let mut forged = direct_site;
                forged.operand.displacement = i64::from(i32::MAX) + 1;
                assert!(!forged.has_supported_profile());
                let mut forged = direct_site;
                forged.operand.base = AddressBase::Register(Register::Rax);
                assert!(!forged.matches_source_bytes(&source, operation));
                let mut forged = direct_site;
                match operation {
                    MemorySiteOperationV2::Load => {
                        forged.destination = Some(if data_register == 6 { 7 } else { 6 });
                    }
                    MemorySiteOperationV2::Store => {
                        forged.source = Some(if data_register == 6 { 7 } else { 6 });
                    }
                }
                assert!(!forged.matches_source_bytes(&source, operation));
                let mut forged = direct_site;
                forged.operand.index = Some(Register::Rax);
                assert!(!forged.has_supported_profile());
                let mut forged = direct_site;
                forged.operand.scale = 2;
                assert!(!forged.has_supported_profile());
                let mut forged = direct_site;
                forged.operand.address_size = od_isa::OperandSize::Dword;
                assert!(!forged.has_supported_profile());
                let mut forged = direct_site;
                forged.operand.size = od_isa::OperandSize::Dword;
                assert!(!forged.has_supported_profile());
                let mut forged = direct_site;
                forged.next_rip += 1;
                assert!(!forged.has_supported_profile());

                for visible in 1..source.len() {
                    let partial = CodeView::new(0x6800, &source[..visible]).unwrap();
                    let direct = if operation == MemorySiteOperationV2::Load {
                        compile_memory_load_block_v2(partial, 0x6800).map(|_| ())
                    } else {
                        compile_memory_store_block_v2(partial, 0x6800).map(|_| ())
                    };
                    let shared_ir = if operation == MemorySiteOperationV2::Load {
                        compile_ir_memory_load_block_v2(partial, 0x6800).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(partial, 0x6800).map(|_| ())
                    };
                    assert!(direct.is_err(), "direct accepted partial {visible}");
                    assert!(shared_ir.is_err(), "IR accepted partial {visible}");
                }
            }
        }
    }

    fn displacement_form(is_load: bool, use_sib: bool, mode: u8, displacement: i32) -> Vec<u8> {
        let opcode = if is_load { 0x8b } else { 0x89 };
        let rm = if use_sib { 4 } else { 0 };
        let mut bytes = vec![0x48, opcode, (mode << 6) | rm];
        if use_sib {
            bytes.push((1 << 6) | (1 << 3)); // scale 2, index RCX, base RAX
        }
        match mode {
            1 => bytes.push(displacement as i8 as u8),
            2 => bytes.extend_from_slice(&displacement.to_le_bytes()),
            _ => unreachable!(),
        }
        bytes
    }

    #[test]
    fn alias_load_rax_from_rax_is_admitted_without_host_side_address_math() {
        let code = CodeView::new(0x3000, &[0x48, 0x8b, 0x00]).unwrap();
        let block = compile_memory_load_block_v2(code, 0x3000).unwrap();
        assert_eq!(block.site().destination(), Some(0));
    }

    #[test]
    fn compiles_only_the_exact_checked_qword_store_form() {
        let bytes = [0x48, 0x89, 0x18]; // MOV [RAX], RBX
        let code = CodeView::new(0x4000, &bytes).unwrap();
        let block = compile_memory_store_block_v2(code, 0x4000).unwrap();
        assert_eq!(block.source_bytes(), &bytes);
        assert_eq!(block.site().operation(), MemorySiteOperationV2::Store);
        assert_eq!(block.site().destination(), None);
        assert_eq!(block.site().source(), Some(3));
        assert_eq!(
            block.compile_flavor(),
            crate::translation::CompileFlavor::S3_20bMemoryStoreV2
        );
        assert!(!block.code().words().is_empty());

        for bytes in [
            vec![0x48, 0x89, 0x18, 0x90],       // trailing instruction
            vec![0x40, 0x48, 0x89, 0x18],       // extra prefix
            vec![0x48, 0x89, 0x1c],             // SIB
            vec![0x48, 0x89, 0x58],             // displacement
            vec![0x49, 0x89, 0xbd, 0, 0, 0, 0], // extended base
        ] {
            let code = CodeView::new(0x5000, &bytes).unwrap();
            assert!(matches!(
                compile_memory_store_block_v2(code, 0x5000),
                Err(MemoryLoadCompileErrorV2::UnsupportedForm)
                    | Err(MemoryLoadCompileErrorV2::Decode)
            ));
        }
        let symmetric_load = [0x48, 0x8b, 0x30];
        assert!(
            compile_memory_load_block_v2(CodeView::new(0x5000, &symmetric_load).unwrap(), 0x5000)
                .is_ok()
        );
    }

    #[test]
    fn active_code_v3_compiles_only_exact_three_instruction_source() {
        let prefix_value = 0x1122_3344_5566_7788_u64;
        let tail_value = 0xCAFE_BABE_1234_5678_u64;
        let mut bytes = vec![0x48, 0xBA]; // MOV RDX, imm64
        bytes.extend_from_slice(&prefix_value.to_le_bytes());
        bytes.extend_from_slice(&[0x48, 0x89, 0x03]); // MOV [RBX], RAX
        bytes.extend_from_slice(&[0x48, 0xB9]); // MOV RCX, imm64
        bytes.extend_from_slice(&tail_value.to_le_bytes());
        let code = CodeView::new(0xA000, &bytes).unwrap();
        let artifact = compile_active_code_block_v3(code, 0xA000).unwrap();
        assert_eq!(artifact.entry_rip(), 0xA000);
        assert_eq!(artifact.source_bytes().as_slice(), bytes);
        assert_eq!(artifact.completed_prefix_value(), prefix_value);
        assert_eq!(artifact.tail_value(), tail_value);
        assert_eq!(artifact.site().guest_rip(), 0xA00A);
        assert_eq!(artifact.site().next_rip(), 0xA00D);
        assert_eq!(artifact.site().source(), Some(0));
        assert_eq!(
            artifact.compile_flavor(),
            crate::translation::CompileFlavor::S3_24aActiveCodeV3
        );
        assert!(!artifact.code().words().is_empty());

        let map = artifact.pc_map();
        let code_len = (artifact.code().words().len() * 4) as u32;
        assert_eq!(map.code_len(), code_len);
        assert_eq!(map.instruction_count(), 3);
        assert_eq!(map.ranges().first().unwrap().range.start(), 0);
        assert_eq!(map.ranges().last().unwrap().range.end(), code_len);
        for (index, range) in map.ranges().iter().enumerate() {
            assert_eq!(range.range.start() % 4, 0, "range {index} start");
            assert_eq!(range.range.end() % 4, 0, "range {index} end");
            assert!(range.range.start() < range.range.end());
            if let Some(next) = map.ranges().get(index + 1) {
                assert_eq!(range.range.end(), next.range.start(), "range {index} gap");
            }
        }
        assert_eq!(
            map.boundaries()
                .iter()
                .map(|boundary| (
                    boundary.rip,
                    boundary.committed_instructions,
                    boundary.instruction_index,
                ))
                .collect::<Vec<_>>(),
            vec![
                (GuestRipBoundary::Fixed(0xA00A), 1, 0),
                (GuestRipBoundary::Fixed(0xA00D), 2, 1),
                (GuestRipBoundary::Fixed(0xA017), 3, 2),
            ]
        );
        let commits: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::CommitInProgress)
            .collect();
        assert_eq!(commits.len(), 3);
        for (commit, boundary, guest_rip, instruction_index, committed_count) in [
            (&commits[0], &map.boundaries()[0], 0xA000, 0, 1),
            (&commits[1], &map.boundaries()[1], 0xA00A, 1, 2),
            (&commits[2], &map.boundaries()[2], 0xA00D, 2, 3),
        ] {
            assert_eq!(commit.guest_rip, GuestRipSource::Fixed(guest_rip));
            assert_eq!(commit.instruction_index, Some(instruction_index));
            assert_eq!(
                commit.snapshot,
                SnapshotAdmissibility::InadmissibleDuringCommit
            );
            assert_eq!(boundary.host_offset, commit.range.end());
            assert_eq!(boundary.phase, StatePhase::Committed);
            assert_eq!(boundary.committed_instructions, committed_count);
            assert_eq!(boundary.snapshot, SnapshotAdmissibility::CanonicalBoundary);
        }
        assert!(map.ranges().iter().any(|range| {
            range.phase == StatePhase::HelperCall
                && range.guest_rip == GuestRipSource::Fixed(0xA00A)
                && range.snapshot == SnapshotAdmissibility::InadmissibleAfterEffect
        }));
        assert!(map.ranges().iter().any(|range| {
            range.phase == StatePhase::AfterHelper
                && range.guest_rip == GuestRipSource::Fixed(0xA00A)
                && range.snapshot == SnapshotAdmissibility::InadmissibleAfterEffect
        }));
        let exits: Vec<_> = map
            .ranges()
            .iter()
            .filter(|range| range.phase == StatePhase::Exit)
            .collect();
        let helper_failure: Vec<_> = exits
            .iter()
            .filter(|range| {
                range.guest_rip == GuestRipSource::Fixed(0xA00A)
                    && range.instruction_index == Some(1)
            })
            .collect();
        assert_eq!(helper_failure.len(), 1);
        assert_eq!(
            helper_failure[0].snapshot,
            SnapshotAdmissibility::InadmissibleAfterEffect
        );
        let invalidated: Vec<_> = exits
            .iter()
            .filter(|range| {
                range.guest_rip == GuestRipSource::Fixed(0xA00D)
                    && range.instruction_index == Some(1)
            })
            .collect();
        assert_eq!(invalidated.len(), 1);
        assert_eq!(
            invalidated[0].snapshot,
            SnapshotAdmissibility::CanonicalBoundary
        );
        let tail_exits: Vec<_> = exits
            .iter()
            .filter(|range| {
                range.guest_rip == GuestRipSource::Fixed(0xA017)
                    && range.instruction_index == Some(2)
            })
            .collect();
        assert_eq!(
            tail_exits.len(),
            3,
            "post-tail path, STOP-after-tail, Complete"
        );
        assert!(
            tail_exits
                .iter()
                .all(|range| range.snapshot == SnapshotAdmissibility::CanonicalBoundary)
        );
        let shared_exits: Vec<_> = exits
            .iter()
            .filter(|range| {
                range.guest_rip == GuestRipSource::CanonicalState
                    && range.instruction_index.is_none()
            })
            .collect();
        assert_eq!(shared_exits.len(), 3, "stop-RIP, STOP, and budget stubs");
        assert!(
            shared_exits
                .iter()
                .all(|range| range.snapshot == SnapshotAdmissibility::CanonicalBoundary)
        );

        for malformed in [
            bytes[..22].to_vec(),
            [bytes.as_slice(), &[0x90]].concat(),
            {
                let mut bad = bytes.clone();
                bad[1] = 0xB8; // prefix no longer writes RDX
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[12] = 0x00; // store no longer has the approved /r form
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[14] = 0xB8; // tail no longer writes RCX
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[0] = 0x4A; // unused REX.X is outside the exact source form
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[0] = 0x4C; // unused REX.R is outside the exact source form
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[0] = 0x4E; // combined unused REX.R/X is rejected too
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[10] = 0x4E; // store's unused REX.R/X bits are rejected
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[13] = 0x4A; // tail's unused REX.X is outside the profile
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[13] = 0x4C; // tail's unused REX.R is outside the profile
                bad
            },
            {
                let mut bad = bytes.clone();
                bad[13] = 0x4E; // combined unused tail REX.R/X is rejected
                bad
            },
        ] {
            assert!(matches!(
                compile_active_code_block_v3(CodeView::new(0xA000, &malformed).unwrap(), 0xA000),
                Err(MemoryLoadCompileErrorV2::UnsupportedForm)
                    | Err(MemoryLoadCompileErrorV2::Decode)
            ));
        }
    }

    #[test]
    fn indexed_no_base_sib_disp32_has_exact_qword_profile_for_all_registers() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for index_register in [0_u8, 1, 2, 3, 5] {
                    for scale in [1_u8, 2, 4, 8] {
                        for displacement in [i32::MIN, -1, 0, 1, i32::MAX] {
                            let scale_bits = scale.trailing_zeros() as u8;
                            let sib = (scale_bits << 6) | (index_register << 3) | 5;
                            let mut source = vec![0x48, opcode, (data_register << 3) | 4, sib];
                            source.extend_from_slice(&displacement.to_le_bytes());
                            let code = CodeView::new(0x2b00, &source).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct = compile_memory_load_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct = compile_memory_store_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_store_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.operand.base, od_isa::long_decode::AddressBase::None);
                            assert_eq!(
                                site.operand.index,
                                od_isa::Register::from_encoding(index_register)
                            );
                            assert_eq!(site.operand.scale, scale);
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            assert_eq!(site.next_rip(), 0x2b08);
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // no-index but not exact 0x25
                vec![0x48, opcode, 0x04, 0x06, 0, 0, 0, 0], // unsupported RSI base
                vec![0x67, 0x48, opcode, 0x34, 0x36, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x48, 0x67, opcode, 0x04, 0x05, 0, 0, 0, 0], // wrong prefix order
            ] {
                let code = CodeView::new(0x2b00, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2b00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2b00).map(|_| ())
                };
                assert!(result.is_err(), "invalid indexed no-base SIB {source:02x?}");
            }
            let complete = [0x48, opcode, 0x04, 0x05, 0, 0, 0, 0];
            for length in 1..8 {
                let code = CodeView::new(0x2b00, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2b00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2b00).map(|_| ())
                };
                assert!(result.is_err(), "partial indexed no-base SIB {length}");
            }
        }
    }

    #[test]
    fn indexed_no_base_rsi_rdi_sib_disp32_has_exact_qword_profile() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for index_register in [6_u8, 7] {
                    for scale in [1_u8, 2, 4, 8] {
                        for displacement in [i32::MIN, -1, 0, 1, i32::MAX] {
                            let sib = (scale.trailing_zeros() as u8) << 6 | index_register << 3 | 5;
                            let mut source = vec![0x48, opcode, (data_register << 3) | 4, sib];
                            source.extend_from_slice(&displacement.to_le_bytes());
                            let code = CodeView::new(0x2b00, &source).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct = compile_memory_load_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct = compile_memory_store_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_store_block_v2(code, 0x2b00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.operand.base, od_isa::long_decode::AddressBase::None);
                            assert_eq!(
                                site.operand.index,
                                od_isa::Register::from_encoding(index_register)
                            );
                            assert_eq!(site.operand.scale, scale);
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            assert_eq!(site.next_rip(), 0x2b08);
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // SIB index=none cannot carry scale 2
                vec![0x67, 0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // addr32 no-base is not this profile
                vec![0x48, 0x67, opcode, 0x04, 0x35, 0, 0, 0, 0], // wrong prefix order
            ] {
                let code = CodeView::new(0x2b00, &source).unwrap();
                let (direct, ir) = if opcode == 0x8b {
                    (
                        compile_memory_load_block_v2(code, 0x2b00).map(|_| ()),
                        compile_ir_memory_load_block_v2(code, 0x2b00).map(|_| ()),
                    )
                } else {
                    (
                        compile_memory_store_block_v2(code, 0x2b00).map(|_| ()),
                        compile_ir_memory_store_block_v2(code, 0x2b00).map(|_| ()),
                    )
                };
                assert!(direct.is_err(), "direct invalid addr64 SIB {source:02x?}");
                assert!(ir.is_err(), "IR invalid addr64 SIB {source:02x?}");
            }
            let complete = [0x48, opcode, 0x04, 0x35, 0, 0, 0, 0];
            for length in 1..8 {
                let code = CodeView::new(0x2b00, &complete[..length]).unwrap();
                let (direct, ir) = if opcode == 0x8b {
                    (
                        compile_memory_load_block_v2(code, 0x2b00).map(|_| ()),
                        compile_ir_memory_load_block_v2(code, 0x2b00).map(|_| ()),
                    )
                } else {
                    (
                        compile_memory_store_block_v2(code, 0x2b00).map(|_| ()),
                        compile_ir_memory_store_block_v2(code, 0x2b00).map(|_| ()),
                    )
                };
                assert!(
                    direct.is_err(),
                    "direct partial addr64 no-base SIB {length}"
                );
                assert!(ir.is_err(), "IR partial addr64 no-base SIB {length}");
            }
        }
    }

    #[test]
    fn address32_indexed_no_base_sib_disp32_has_exact_qword_profile_for_all_registers() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for index_register in [0_u8, 1, 2, 3, 5] {
                    for scale in [1_u8, 2, 4, 8] {
                        for displacement in [i32::MIN, -1, 0, 1, i32::MAX] {
                            let sib = (scale.trailing_zeros() as u8) << 6 | index_register << 3 | 5;
                            let mut source =
                                vec![0x67, 0x48, opcode, (data_register << 3) | 4, sib];
                            source.extend_from_slice(&displacement.to_le_bytes());
                            let code = CodeView::new(0x2c00, &source).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct = compile_memory_load_block_v2(code, 0x2c00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2c00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct = compile_memory_store_block_v2(code, 0x2c00)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_store_block_v2(code, 0x2c00)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.operand.base, od_isa::long_decode::AddressBase::None);
                            assert_eq!(
                                site.operand.index,
                                od_isa::Register::from_encoding(index_register)
                            );
                            assert_eq!(site.operand.scale, scale);
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            assert_eq!(site.next_rip(), 0x2c09);
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // non-exact no-index SIB
                vec![0x67, 0x48, opcode, 0x34, 0x36, 0, 0, 0, 0], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x04, 0x05, 0, 0, 0, 0, 0], // extra byte
                vec![0x48, 0x8b, 0x04, 0x65, 0, 0, 0, 0],         // invalid addr64 no-index scale
                vec![0x48, 0x67, opcode, 0x04, 0x05, 0, 0, 0, 0], // wrong prefix order
                vec![0x67, 0x48, opcode, 0x05, 0, 0, 0, 0],       // direct rm=101
            ] {
                let code = CodeView::new(0x2c00, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2c00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2c00).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "invalid address32 indexed no-base {source:02x?}"
                );
            }
            let complete = [0x67, 0x48, opcode, 0x04, 0x05, 0, 0, 0, 0];
            for length in 1..9 {
                let code = CodeView::new(0x2c00, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2c00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2c00).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "partial address32 indexed no-base {length}"
                );
            }
        }
    }

    #[test]
    fn address32_indexed_no_base_rsi_rdi_sib_disp32_has_exact_qword_profile() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for index_register in [6_u8, 7] {
                    for scale in [1_u8, 2, 4, 8] {
                        for displacement in [i32::MIN, -1, 0, 1, i32::MAX] {
                            let sib = (scale.trailing_zeros() as u8) << 6 | index_register << 3 | 5;
                            let mut source =
                                vec![0x67, 0x48, opcode, (data_register << 3) | 4, sib];
                            source.extend_from_slice(&displacement.to_le_bytes());
                            let code = CodeView::new(0x2c40, &source).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let direct = compile_memory_load_block_v2(code, 0x2c40)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2c40)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let direct = compile_memory_store_block_v2(code, 0x2c40)
                                        .unwrap_or_else(|error| {
                                            panic!("direct {source:02x?}: {error:?}")
                                        });
                                    let ir = compile_ir_memory_store_block_v2(code, 0x2c40)
                                        .unwrap_or_else(|error| {
                                            panic!("IR {source:02x?}: {error:?}")
                                        });
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.operand.base, od_isa::long_decode::AddressBase::None);
                            assert_eq!(
                                site.operand.index,
                                od_isa::Register::from_encoding(index_register)
                            );
                            assert_eq!(site.operand.scale, scale);
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            assert_eq!(site.next_rip(), 0x2c49);
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // non-unit scale with no index
                vec![0x67, 0x48, opcode, 0x34, 0x75, 0, 0, 0, 0], // RSI data register is unsupported
                vec![0x66, 0x67, 0x48, opcode, 0x04, 0x75, 0, 0, 0, 0], // extra prefix
                vec![0x48, 0x67, opcode, 0x04, 0x75, 0, 0, 0, 0], // wrong prefix order
            ] {
                let code = CodeView::new(0x2c40, &source).unwrap();
                let direct = if opcode == 0x8b {
                    compile_memory_load_block_v2(code, 0x2c40).map(|_| ())
                } else {
                    compile_memory_store_block_v2(code, 0x2c40).map(|_| ())
                };
                let ir = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2c40).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2c40).map(|_| ())
                };
                assert!(direct.is_err(), "unexpected direct admission {source:02x?}");
                assert!(ir.is_err(), "unexpected IR admission {source:02x?}");
            }
            let complete = [0x67, 0x48, opcode, 0x04, 0x75, 0, 0, 0, 0];
            for visible in 1..complete.len() {
                let code = CodeView::new(0x2c40, &complete[..visible]).unwrap();
                let direct = if opcode == 0x8b {
                    compile_memory_load_block_v2(code, 0x2c40).map(|_| ())
                } else {
                    compile_memory_store_block_v2(code, 0x2c40).map(|_| ())
                };
                let ir = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2c40).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2c40).map(|_| ())
                };
                assert!(
                    direct.is_err(),
                    "partial direct window {visible}/9 admitted"
                );
                assert!(ir.is_err(), "partial IR window {visible}/9 admitted");
            }
        }
    }

    #[test]
    fn indexed_rbp_sib_without_displacement_is_exact_for_all_data_and_base_registers() {
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0..6_u8 {
                for base_register in 0..4_u8 {
                    for scale in [1_u8, 2, 4, 8] {
                        let sib = (scale.trailing_zeros() as u8) << 6 | (5 << 3) | base_register;
                        let source = [0x48, opcode, (data_register << 3) | 4, sib];
                        let code = CodeView::new(0x2d00, &source).unwrap();
                        let site = match operation {
                            MemorySiteOperationV2::Load => {
                                let direct = compile_memory_load_block_v2(code, 0x2d00).unwrap();
                                let ir = compile_ir_memory_load_block_v2(code, 0x2d00).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                            MemorySiteOperationV2::Store => {
                                let direct = compile_memory_store_block_v2(code, 0x2d00).unwrap();
                                let ir = compile_ir_memory_store_block_v2(code, 0x2d00).unwrap();
                                assert_eq!(ir.site(), direct.site());
                                ir.site().clone()
                            }
                        };
                        assert!(site.has_supported_profile(), "{source:02x?}");
                        assert!(site.matches_source_bytes(&source, operation));
                        assert_eq!(site.next_rip(), 0x2d04);
                        assert_eq!(site.operand.address_size, od_isa::OperandSize::Qword);
                        assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                        assert_eq!(
                            site.operand.base,
                            od_isa::long_decode::AddressBase::Register(
                                od_isa::Register::from_encoding(base_register).unwrap()
                            )
                        );
                        assert_eq!(site.operand.index, Some(od_isa::Register::Rbp));
                        assert_eq!(site.operand.scale, scale);
                        assert_eq!(site.operand.displacement, 0);
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x48, opcode, 0x04, 0x2d],       // no-base plus RBP index
                vec![0x48, opcode, 0x04, 0x66],       // RSI base with no-index scale alias
                vec![0x48, opcode, 0x04, 0x67],       // RDI base with no-index scale alias
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x48, 0x67, opcode, 0x04, 0x28], // reordered prefix
            ] {
                let code = CodeView::new(0x2d00, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2d00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2d00).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "unsupported SIB neighbor admitted {source:02x?}"
                );
            }
            let complete = [0x48, opcode, 0x04, 0x28];
            for length in 1..4 {
                let code = CodeView::new(0x2d00, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2d00).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2d00).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "partial RBP-index SIB admitted at {length}"
                );
            }
        }

        // SIB index=100 is still the no-index encoding; it never aliases RBP.
        let no_index = [0x48, 0x8b, 0x04, 0x24];
        let site =
            compile_ir_memory_load_block_v2(CodeView::new(0x2d00, &no_index).unwrap(), 0x2d00)
                .unwrap()
                .site()
                .clone();
        assert_eq!(
            site.operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        );
        assert_eq!(site.operand.index, None);
    }

    #[test]
    fn indexed_rbp_sib_displacements_are_exact_for_all_data_bases_and_scales() {
        let displacements = [
            (1_u8, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (4, i32::MIN),
            (4, -1),
            (4, 0),
            (4, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b_u8, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0..=5_u8 {
                for base in 0..4_u8 {
                    for scale in [1_u8, 2, 4, 8] {
                        for &(width, displacement) in &displacements {
                            let mode = if width == 1 { 1 } else { 2 };
                            let sib = (scale.trailing_zeros() as u8) << 6 | (5 << 3) | base;
                            let mut source = vec![0x48, opcode, (mode << 6) | (data << 3) | 4, sib];
                            if width == 1 {
                                source.push(displacement as u8);
                            } else {
                                source.extend_from_slice(&displacement.to_le_bytes());
                            }
                            let code = CodeView::new(0x2d20, &source).unwrap();
                            let (ir_site, direct_site, ir_source) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2d20).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x2d20).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x2d20).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x2d20).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                            };
                            assert_eq!(ir_site, direct_site, "{source:02x?}");
                            assert_eq!(ir_source, source, "{source:02x?}");
                            assert!(ir_site.has_supported_profile(), "{source:02x?}");
                            assert!(ir_site.matches_source_bytes(&source, operation));
                            assert_eq!(ir_site.guest_rip(), 0x2d20);
                            assert_eq!(ir_site.next_rip(), 0x2d20 + source.len() as u64);
                            assert_eq!(
                                ir_site.operand().base,
                                od_isa::long_decode::AddressBase::Register(
                                    Register::from_encoding(base).unwrap()
                                )
                            );
                            assert_eq!(ir_site.operand().index, Some(Register::Rbp));
                            assert_eq!(ir_site.operand().scale, scale);
                            assert_eq!(ir_site.operand().displacement, i64::from(displacement));
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x48, opcode, 0x74, 0x25, 0], // RSI data is outside this profile
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // invalid no-index scale
                vec![0x67, 0x48, opcode, 0x74, 0x34, 0], // unsupported RSI data register
                vec![0x48, 0x67, opcode, 0x44, 0x28, 0], // reordered prefix
            ] {
                let code = CodeView::new(0x2d20, &source).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2d20).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2d20).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "unsupported SIB neighbor admitted {source:02x?}"
                );
            }
            for complete in [[0x48, opcode, 0x44, 0x28, 0]] {
                for length in 1..complete.len() {
                    let code = CodeView::new(0x2d20, &complete[..length]).unwrap();
                    let result = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(code, 0x2d20).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(code, 0x2d20).map(|_| ())
                    };
                    assert!(result.is_err(), "partial disp8 admitted at {length}");
                }
            }
            let complete = [0x48, opcode, 0x84, 0x28, 0, 0, 0, 0];
            for length in 1..complete.len() {
                let code = CodeView::new(0x2d20, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2d20).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2d20).map(|_| ())
                };
                assert!(result.is_err(), "partial disp32 admitted at {length}");
            }
        }
    }
    #[test]
    fn rsp_base_rbp_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, [0_i32].as_slice()),
            (1, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for scale_shift in 0..4_u8 {
                    for (mode, displacements) in displacement_cases {
                        for &displacement in displacements {
                            let mut bytes = vec![
                                0x48,
                                opcode,
                                (mode << 6) | (data_register << 3) | 4,
                                (scale_shift << 6) | (5 << 3) | 4,
                            ];
                            match mode {
                                0 => {}
                                1 => bytes.push(displacement as u8),
                                2 => bytes.extend_from_slice(&displacement.to_le_bytes()),
                                _ => unreachable!(),
                            }
                            let code = CodeView::new(0x2df0, &bytes).unwrap();
                            let site = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2df0).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x2df0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x2df0).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x2df0).unwrap();
                                    assert_eq!(ir.site(), direct.site());
                                    ir.site().clone()
                                }
                            };
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&bytes, operation));
                            assert_eq!(
                                site.operand().base,
                                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                            );
                            assert_eq!(site.operand().index, Some(od_isa::Register::Rbp));
                            assert_eq!(site.operand().scale, 1 << scale_shift);
                            assert_eq!(site.operand().displacement, i64::from(displacement));
                            assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(site.next_rip(), 0x2df0 + bytes.len() as u64);
                            let mut state = od_isa::LongModeState::default();
                            state.gpr[4] = 0x0000_0000_1000_0000;
                            state.gpr[5] = 0x0000_0000_0000_0123;
                            let expected = state.gpr[4]
                                .wrapping_add(state.gpr[5].wrapping_mul(1_u64 << scale_shift))
                                .wrapping_add_signed(i64::from(displacement));
                            assert_eq!(
                                site.operand().access_address_48(&state, site.next_rip()),
                                Ok(expected)
                            );
                        }
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x48, opcode, 0x04, 0x2d],       // no-base, RBP index
                vec![0x48, opcode, 0x74, 0x25, 0],    // RSI data is outside this profile
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x66, 0x48, opcode, 0x04, 0x2c], // extra prefix
            ] {
                let code = CodeView::new(0x2df0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2df0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2df0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "unsupported RSP/RBP SIB admitted: {bytes:02x?}"
                );
            }
        }
    }

    #[test]
    fn rsp_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, [0_i32].as_slice()),
            (1, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for index_register in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for (mode, displacements) in displacement_cases {
                            for &displacement in displacements {
                                let mut bytes = vec![
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data_register << 3) | 4,
                                    (scale_shift << 6) | (index_register << 3) | 4,
                                ];
                                match mode {
                                    0 => {}
                                    1 => bytes.push(displacement as u8),
                                    2 => bytes.extend_from_slice(&displacement.to_le_bytes()),
                                    _ => unreachable!(),
                                }
                                let code = CodeView::new(0x2df0, &bytes).unwrap();
                                let site = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let ir = compile_ir_memory_load_block_v2(code, 0x2df0)
                                            .unwrap_or_else(|error| {
                                                panic!("load rejected {bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x2df0).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        ir.site().clone()
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x2df0).unwrap();
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x2df0).unwrap();
                                        assert_eq!(ir.site(), direct.site());
                                        ir.site().clone()
                                    }
                                };
                                assert!(site.has_supported_profile());
                                assert!(site.matches_source_bytes(&bytes, operation));
                                assert_eq!(
                                    site.operand().base,
                                    od_isa::long_decode::AddressBase::Register(
                                        od_isa::Register::Rsp
                                    )
                                );
                                assert_eq!(
                                    site.operand().index,
                                    Some(if index_register == 6 {
                                        od_isa::Register::Rsi
                                    } else {
                                        od_isa::Register::Rdi
                                    })
                                );
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                                let mut state = od_isa::LongModeState::default();
                                state.gpr[4] = 0x0000_0000_1000_0000;
                                state.gpr[usize::from(index_register)] = 0x0000_0000_0000_0103;
                                let expected = state.gpr[4]
                                    .wrapping_add(
                                        state.gpr[usize::from(index_register)]
                                            .wrapping_mul(1 << scale_shift),
                                    )
                                    .wrapping_add_signed(i64::from(displacement));
                                assert_eq!(
                                    site.operand().access_address_48(&state, site.next_rip()),
                                    Ok(expected)
                                );
                            }
                        }
                    }
                }
            }
        }
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x48, opcode, 0x04, 0x2d],       // no-base/RBP index
                vec![0x48, opcode, 0x74, 0x25, 0],    // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x34, 0x34], // unsupported RSI data register
                vec![0x66, 0x48, opcode, 0x04, 0x2c], // extra prefix
            ] {
                let code = CodeView::new(0x2df0, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2df0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2df0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "unsupported RSP/RSI-RDI SIB admitted: {bytes:02x?}"
                );
            }
            let complete = [0x48, opcode, 0x84, 0x74, 0x7f, 1, 2, 3];
            for length in 1..complete.len() {
                let code = CodeView::new(0x2df0, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2df0).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2df0).map(|_| ())
                };
                assert!(
                    result.is_err(),
                    "partial RSP+RSI/RDI disp32 admitted at {length}"
                );
            }
        }
    }

    #[test]
    fn address32_rsp_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (0_u8, [0_i32].as_slice()),
            (1, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for index_register in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for (mode, displacements) in displacement_cases {
                            for &displacement in displacements {
                                let mut bytes = vec![
                                    0x67,
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data_register << 3) | 4,
                                    (scale_shift << 6) | (index_register << 3) | 4,
                                ];
                                match mode {
                                    0 => {}
                                    1 => bytes.push(displacement as u8),
                                    2 => bytes.extend_from_slice(&displacement.to_le_bytes()),
                                    _ => unreachable!(),
                                }
                                let code = CodeView::new(0x2e80, &bytes).unwrap();
                                let (site, direct_site) = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let ir = compile_ir_memory_load_block_v2(code, 0x2e80)
                                            .unwrap_or_else(|error| {
                                                panic!("load rejected {bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x2e80).unwrap();
                                        (ir.site().clone(), direct.site().clone())
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir = compile_ir_memory_store_block_v2(code, 0x2e80)
                                            .unwrap_or_else(|error| {
                                                panic!("store rejected {bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x2e80).unwrap();
                                        (ir.site().clone(), direct.site().clone())
                                    }
                                };
                                assert_eq!(site, direct_site);
                                assert!(site.has_supported_profile());
                                assert!(site.matches_source_bytes(&bytes, operation));
                                assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand().base,
                                    od_isa::long_decode::AddressBase::Register(
                                        od_isa::Register::Rsp
                                    )
                                );
                                assert_eq!(
                                    site.operand().index,
                                    Some(if index_register == 6 {
                                        od_isa::Register::Rsi
                                    } else {
                                        od_isa::Register::Rdi
                                    })
                                );
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                                assert_eq!(site.next_rip(), 0x2e80 + bytes.len() as u64);
                                let mut state = od_isa::LongModeState::default();
                                state.gpr[4] = 0xdead_beef_ffff_fff0;
                                state.gpr[usize::from(index_register)] = 0xcafe_babe_8000_0003;
                                let expected = (state.gpr[4] as u32)
                                    .wrapping_add(
                                        (state.gpr[usize::from(index_register)] as u32)
                                            .wrapping_mul(1 << scale_shift),
                                    )
                                    .wrapping_add(displacement as u32)
                                    as u64;
                                assert_eq!(
                                    site.operand().access_address_48(&state, site.next_rip()),
                                    Ok(expected)
                                );
                            }
                        }
                    }
                }
            }
            for bytes in [
                vec![0x67, 0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // no-index non-unit scale
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0],       // addr64 invalid no-index scale
                vec![0x67, 0x48, opcode, 0x34, 0x34], // RSI data is outside data whitelist
                vec![0x66, 0x67, 0x48, opcode, 0x04, 0x34], // extra prefix
            ] {
                let code = CodeView::new(0x2e80, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2e80).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2e80).map(|_| ())
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
            let complete = [0x67, 0x48, opcode, 0x84, 0x74, 0x7f, 1, 2, 3];
            for length in 1..complete.len() {
                let code = CodeView::new(0x2e80, &complete[..length]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2e80).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2e80).map(|_| ())
                };
                assert!(result.is_err(), "partial instruction admitted at {length}");
            }
        }
    }

    #[test]
    fn address32_rbp_base_rsi_rdi_index_sib_has_exact_qword_profile() {
        let displacement_cases = [
            (1_u8, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for index_register in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for (mode, displacements) in displacement_cases {
                            for &displacement in displacements {
                                let mut bytes = vec![
                                    0x67,
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data_register << 3) | 4,
                                    (scale_shift << 6) | (index_register << 3) | 5,
                                ];
                                match mode {
                                    1 => bytes.push(displacement as u8),
                                    2 => bytes.extend_from_slice(&displacement.to_le_bytes()),
                                    _ => unreachable!(),
                                }
                                let code = CodeView::new(0x2f20, &bytes).unwrap();
                                let (site, direct_site, copied) = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let ir = compile_ir_memory_load_block_v2(code, 0x2f20)
                                            .unwrap_or_else(|error| {
                                                panic!("load rejected {bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x2f20).unwrap();
                                        (
                                            ir.site().clone(),
                                            direct.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir = compile_ir_memory_store_block_v2(code, 0x2f20)
                                            .unwrap_or_else(|error| {
                                                panic!("store rejected {bytes:02x?}: {error:?}")
                                            });
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x2f20).unwrap();
                                        (
                                            ir.site().clone(),
                                            direct.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                };
                                assert_eq!(site, direct_site);
                                assert_eq!(copied, bytes);
                                assert!(site.has_supported_profile());
                                assert!(site.matches_source_bytes(&bytes, operation));
                                assert_eq!(site.next_rip(), 0x2f20 + bytes.len() as u64);
                                assert_eq!(site.operand().address_size, od_isa::OperandSize::Dword);
                                assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                                assert_eq!(
                                    site.operand().base,
                                    od_isa::long_decode::AddressBase::Register(
                                        od_isa::Register::Rbp
                                    )
                                );
                                assert_eq!(
                                    site.operand().index,
                                    Some(if index_register == 6 {
                                        od_isa::Register::Rsi
                                    } else {
                                        od_isa::Register::Rdi
                                    })
                                );
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                                let mut state = od_isa::LongModeState::default();
                                state.gpr[5] = 0xdead_beef_ffff_fff0;
                                state.gpr[usize::from(index_register)] = 0xcafe_babe_8000_0003;
                                let expected = ((state.gpr[5] as u32)
                                    .wrapping_add(
                                        (state.gpr[usize::from(index_register)] as u32)
                                            .wrapping_mul(1 << scale_shift),
                                    )
                                    .wrapping_add(displacement as u32))
                                    as u64;
                                assert_eq!(
                                    site.operand().access_address_48(&state, site.next_rip()),
                                    Ok(expected)
                                );
                            }
                        }
                    }
                }
            }

            for (mode, displacement, full_length) in
                [(1_u8, i8::MIN as i32, 6_usize), (2, i32::MIN, 9)]
            {
                let mut full = vec![0x67, 0x48, opcode, (mode << 6) | 4, (6 << 3) | 5];
                if mode == 1 {
                    full.push(displacement as u8);
                } else {
                    full.extend_from_slice(&displacement.to_le_bytes());
                }
                assert_eq!(full.len(), full_length);
                for visible in 1..full_length {
                    let code = CodeView::new(0x2f20, &full[..visible]).unwrap();
                    let result = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(code, 0x2f20).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(code, 0x2f20).map(|_| ())
                    };
                    assert!(result.is_err(), "partial window {visible}/{full_length}");
                }
            }
            for bytes in [
                vec![0x67, 0x48, opcode, 0x04, 0x65, 0, 0, 0, 0], // no-index non-unit scale
                vec![0x67, 0x48, opcode, 0x34, 0x35, 0, 0, 0, 0], // RSI data is unsupported
                vec![0x67, 0x48, opcode, 0x74, 0x2d, 0],          // RSI data is unsupported
                vec![0x66, 0x67, 0x48, opcode, 0x44, 0x75, 0],    // extra prefix
            ] {
                let code = CodeView::new(0x2f20, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2f20).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2f20).map(|_| ())
                };
                assert!(result.is_err(), "unexpectedly admitted {bytes:02x?}");
            }
        }
    }

    #[test]
    fn rbp_base_rbp_index_sib_displacements_have_exact_qword_profile() {
        let cases = [
            (1_u8, 5_usize, vec![i8::MIN as i32, -1, 0, i8::MAX as i32]),
            (2_u8, 8_usize, vec![i32::MIN, -1, 0, 1, i32::MAX]),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for scale_shift in 0..4_u8 {
                    for (mode, length, displacements) in &cases {
                        for &displacement in displacements {
                            let mut bytes = vec![
                                0x48,
                                opcode,
                                (*mode << 6) | (data_register << 3) | 4,
                                (scale_shift << 6) | (5 << 3) | 5,
                            ];
                            if *mode == 1 {
                                bytes.push(displacement as u8);
                            } else {
                                bytes.extend_from_slice(&displacement.to_le_bytes());
                            }
                            assert_eq!(bytes.len(), *length);
                            let code = CodeView::new(0x2e40, &bytes).unwrap();
                            let (site, direct_site, source) = match operation {
                                MemorySiteOperationV2::Load => {
                                    let ir = compile_ir_memory_load_block_v2(code, 0x2e40).unwrap();
                                    let direct =
                                        compile_memory_load_block_v2(code, 0x2e40).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                                MemorySiteOperationV2::Store => {
                                    let ir =
                                        compile_ir_memory_store_block_v2(code, 0x2e40).unwrap();
                                    let direct =
                                        compile_memory_store_block_v2(code, 0x2e40).unwrap();
                                    (
                                        ir.site().clone(),
                                        direct.site().clone(),
                                        ir.source_bytes().to_vec(),
                                    )
                                }
                            };
                            assert_eq!(site, direct_site);
                            assert_eq!(source, bytes);
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&bytes, operation));
                            assert_eq!(site.operand().base, AddressBase::Register(Register::Rbp));
                            assert_eq!(site.operand().index, Some(Register::Rbp));
                            assert_eq!(site.operand().scale, 1 << scale_shift);
                            assert_eq!(site.operand().displacement, i64::from(displacement));
                            assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                            assert_eq!(site.next_rip(), 0x2e40 + *length as u64);

                            // For data=RBP the effective address and store
                            // payload are both derived from the old register.
                            let mut state = od_isa::LongModeState::default();
                            state.gpr[5] = 0x9000_0000;
                            let expected = state.gpr[5]
                                .wrapping_add(state.gpr[5].wrapping_mul(1_u64 << scale_shift))
                                .wrapping_add_signed(i64::from(displacement));
                            assert_eq!(
                                site.operand().access_address_48(&state, site.next_rip()),
                                Ok(expected)
                            );
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            // In mod=00, SIB base=101 means no base, not RBP.
            let no_base = [0x48, opcode, 0x04, 0x2d, 0, 0, 0, 0];
            for partial in 1..8 {
                let view = CodeView::new(0x2e40, &no_base[..partial]).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(view, 0x2e40).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(view, 0x2e40).map(|_| ())
                };
                assert!(result.is_err(), "partial/no-base form admitted: {partial}");
            }
            for bytes in [
                vec![0x48, opcode, 0x74, 0x25, 0], // RSI data is outside this profile
                vec![0x67, 0x48, opcode, 0x74, 0x36, 0], // address32 unsupported RSI data register
                vec![0x66, 0x48, opcode, 0x44, 0x2d, 0], // extra prefix
            ] {
                let view = CodeView::new(0x2e40, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(view, 0x2e40).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(view, 0x2e40).map(|_| ())
                };
                assert!(result.is_err(), "unsupported form admitted: {bytes:02x?}");
            }
        }
    }

    #[test]
    fn rbp_base_rsi_rdi_index_sib_displacements_have_exact_qword_profile() {
        let displacement_cases = [
            (1_u8, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data_register in 0_u8..=5 {
                for index_register in [6_u8, 7] {
                    for scale_shift in 0..4_u8 {
                        for (mode, displacements) in displacement_cases {
                            for &displacement in displacements {
                                let mut bytes = vec![
                                    0x48,
                                    opcode,
                                    (mode << 6) | (data_register << 3) | 4,
                                    (scale_shift << 6) | (index_register << 3) | 5,
                                ];
                                if mode == 1 {
                                    bytes.push(displacement as u8);
                                } else {
                                    bytes.extend_from_slice(&displacement.to_le_bytes());
                                }
                                let code = CodeView::new(0x2e80, &bytes).unwrap();
                                let (site, direct_site, source) = match operation {
                                    MemorySiteOperationV2::Load => {
                                        let ir =
                                            compile_ir_memory_load_block_v2(code, 0x2e80).unwrap();
                                        let direct =
                                            compile_memory_load_block_v2(code, 0x2e80).unwrap();
                                        (
                                            ir.site().clone(),
                                            direct.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                    MemorySiteOperationV2::Store => {
                                        let ir =
                                            compile_ir_memory_store_block_v2(code, 0x2e80).unwrap();
                                        let direct =
                                            compile_memory_store_block_v2(code, 0x2e80).unwrap();
                                        (
                                            ir.site().clone(),
                                            direct.site().clone(),
                                            ir.source_bytes().to_vec(),
                                        )
                                    }
                                };
                                assert_eq!(site, direct_site, "{bytes:02x?}");
                                assert_eq!(source, bytes, "{bytes:02x?}");
                                assert!(site.has_supported_profile(), "{bytes:02x?}");
                                assert!(site.matches_source_bytes(&bytes, operation));
                                assert_eq!(
                                    site.operand().base,
                                    AddressBase::Register(Register::Rbp)
                                );
                                assert_eq!(
                                    site.operand().index,
                                    Some(Register::from_encoding(index_register).unwrap())
                                );
                                assert_eq!(site.operand().scale, 1 << scale_shift);
                                assert_eq!(site.operand().displacement, i64::from(displacement));
                                assert_eq!(site.operand().address_size, od_isa::OperandSize::Qword);
                                assert_eq!(site.operand().size, od_isa::OperandSize::Qword);
                                assert_eq!(site.next_rip(), 0x2e80 + bytes.len() as u64);
                                let mut state = od_isa::LongModeState::default();
                                state.gpr[5] = 0x1000;
                                state.gpr[usize::from(index_register)] = 0x1_0000_0103;
                                let expected = state.gpr[5]
                                    .wrapping_add(
                                        state.gpr[usize::from(index_register)]
                                            .wrapping_mul(1 << scale_shift),
                                    )
                                    .wrapping_add_signed(i64::from(displacement));
                                assert_eq!(
                                    site.operand().access_address_48(&state, site.next_rip()),
                                    Ok(expected),
                                    "{bytes:02x?}"
                                );
                            }
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            // Address32 no-base RSI-index remains outside this addr64 profile;
            // addr64 index=none with non-unit scale is a separate invalid shape.
            for bytes in [
                vec![0x48, opcode, 0x04, 0x65, 0, 0, 0, 0],
                vec![0x67, 0x48, opcode, 0x74, 0x36, 0],
                vec![0x66, 0x48, opcode, 0x44, 0x75, 0],
            ] {
                let code = CodeView::new(0x2e80, &bytes).unwrap();
                let result = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, 0x2e80).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, 0x2e80).map(|_| ())
                };
                assert!(result.is_err(), "unsupported form admitted: {bytes:02x?}");
            }
            for complete in [
                vec![0x48, opcode, 0x44, 0x75, 0x7f],
                vec![0x48, opcode, 0x84, 0xf5, 1, 2, 3, 4],
            ] {
                for visible in 1..complete.len() {
                    let code = CodeView::new(0x2e80, &complete[..visible]).unwrap();
                    let result = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(code, 0x2e80).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(code, 0x2e80).map(|_| ())
                    };
                    assert!(
                        result.is_err(),
                        "partial disp SIB form admitted at {visible}: {complete:02x?}"
                    );
                }
            }
        }
    }

    #[test]
    fn runtime_rejects_qword_forged_address32_indexed_lengths() {
        for (mode, displacement) in [
            (1_u8, vec![0x7f]),
            (2, 0x1234_5678_i32.to_le_bytes().to_vec()),
        ] {
            let mut source = vec![0x67, 0x48, 0x8b, (mode << 6) | 0x04, 0x08];
            source.extend_from_slice(&displacement);
            let code = CodeView::new(0x5d00, &source).unwrap();
            let valid = compile_memory_load_block_v2(code, 0x5d00).unwrap();
            assert_eq!(
                valid.site().operand.address_size,
                od_isa::OperandSize::Dword
            );
            assert!(valid.site().has_supported_profile());
            assert!(
                valid
                    .site()
                    .matches_source_bytes(&source, MemorySiteOperationV2::Load)
            );

            let mut forged = *valid.site();
            forged.operand.address_size = od_isa::OperandSize::Qword;
            assert!(
                !forged.has_supported_profile(),
                "forged Qword site admitted for length {}: {source:02x?}",
                source.len()
            );
            assert!(!forged.matches_source_bytes(&source, MemorySiteOperationV2::Load));
        }
    }

    #[test]
    fn address32_low_base_no_index_sib_has_exact_qword_profile() {
        let displacements = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in 0_u8..=3 {
                for data in 0_u8..=5 {
                    for &(mode, displacement) in &displacements {
                        let mut source = vec![
                            0x67,
                            0x48,
                            opcode,
                            (mode << 6) | (data << 3) | 4,
                            0x20 | base,
                        ];
                        match mode {
                            0 => {}
                            1 => source.push(displacement as u8),
                            _ => source.extend_from_slice(&displacement.to_le_bytes()),
                        }
                        let code = CodeView::new(0x5e00, &source).unwrap();
                        let direct_site = if opcode == 0x8b {
                            *compile_memory_load_block_v2(code, 0x5e00).unwrap().site()
                        } else {
                            *compile_memory_store_block_v2(code, 0x5e00).unwrap().site()
                        };
                        let ir_site = if opcode == 0x8b {
                            *compile_ir_memory_load_block_v2(code, 0x5e00)
                                .unwrap()
                                .site()
                        } else {
                            *compile_ir_memory_store_block_v2(code, 0x5e00)
                                .unwrap()
                                .site()
                        };
                        for site in [&direct_site, &ir_site] {
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand.base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(site.operand.index, None);
                            assert_eq!(site.operand.scale, 1);

                            let mut state = od_isa::LongModeState {
                                rip: 0x5e00,
                                ..Default::default()
                            };
                            state.gpr[usize::from(base)] = 0xdead_beef_0000_5000 + u64::from(base);
                            let expected = u64::from(
                                (state.gpr[usize::from(base)] as u32)
                                    .wrapping_add(displacement as u32),
                            );
                            assert_eq!(
                                site.operand.access_address_48(&state, site.next_rip()),
                                Ok(expected),
                                "{source:02x?}"
                            );
                        }
                        for visible in 1..source.len() {
                            let partial = CodeView::new(0x5e00, &source[..visible]).unwrap();
                            let result = if opcode == 0x8b {
                                compile_memory_load_block_v2(partial, 0x5e00).map(|_| ())
                            } else {
                                compile_memory_store_block_v2(partial, 0x5e00).map(|_| ())
                            };
                            assert!(result.is_err(), "partial source admitted: {source:02x?}");
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x67, 0x48, 0x8b, 0x04, 0x60], // absent index with non-unit scale
            vec![0x67, 0x48, 0x8b, 0x34, 0x20], // RSI is outside data-register profile
            vec![0x66, 0x67, 0x48, 0x8b, 0x04, 0x20], // extra prefix
        ] {
            let code = CodeView::new(0x5e00, &source).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5e00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5e00).is_err());
        }

        let first = [0x67, 0x48, 0x8b, 0x84, 0x20, 0, 0, 0, 1];
        let second = [0x67, 0x48, 0x8b, 0x84, 0x20, 0, 0, 0, 2];
        let first = compile_ir_memory_load_block_v2(CodeView::new(0x5e00, &first).unwrap(), 0x5e00)
            .unwrap();
        let second =
            compile_ir_memory_load_block_v2(CodeView::new(0x5e00, &second).unwrap(), 0x5e00)
                .unwrap();
        assert_ne!(first.source_bytes(), second.source_bytes());
        let state = od_isa::LongModeState {
            gpr: [0x1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            first
                .site()
                .operand
                .access_address_48(&state, first.site().next_rip()),
            Ok(0x0100_1000)
        );
        assert_eq!(
            second
                .site()
                .operand
                .access_address_48(&state, second.site().next_rip()),
            Ok(0x0200_1000)
        );
    }

    #[test]
    fn low_and_rsi_rdi_base_no_index_sib_have_exact_qword_profiles() {
        let displacements = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for base in [0_u8, 1, 2, 3, 6, 7] {
                for data in 0_u8..=5 {
                    for &(mode, displacement) in &displacements {
                        let mut source =
                            vec![0x48, opcode, (mode << 6) | (data << 3) | 4, 0x20 | base];
                        match mode {
                            0 => {}
                            1 => source.push(displacement as u8),
                            _ => source.extend_from_slice(&displacement.to_le_bytes()),
                        }
                        let code = CodeView::new(0x5d00, &source).unwrap();
                        let mut decode_memory = CodeMemory::full_code(code);
                        let decoded = long_decode::decode(
                            &mut decode_memory,
                            0x5d00,
                            AccessContext::default(),
                        )
                        .unwrap();
                        assert_eq!(
                            decoded.length as usize,
                            source.len(),
                            "decoder length mismatch for {source:02x?}"
                        );
                        match opcode {
                            0x8b => assert!(
                                crate::block::is_checked_qword_load_form(decoded),
                                "checked load form rejected {source:02x?}: {:?}",
                                decoded.operation
                            ),
                            0x89 => assert!(
                                crate::block::is_checked_qword_store_form(decoded),
                                "checked store form rejected {source:02x?}: {:?}",
                                decoded.operation
                            ),
                            _ => unreachable!(),
                        }
                        let (decoded_register, decoded_operand) = match decoded.operation {
                            Operation::Mov {
                                dst: Operand::Register(register),
                                src: Operand::Memory(operand),
                            } if opcode == 0x8b => (register.register() as u8, operand),
                            Operation::Mov {
                                dst: Operand::Memory(operand),
                                src: Operand::Register(register),
                            } if opcode == 0x89 => (register.register() as u8, operand),
                            _ => panic!("unexpected decoded operation for {source:02x?}"),
                        };
                        let decoded_site = MemorySiteV2 {
                            instruction_index: 0,
                            guest_rip: 0x5d00,
                            next_rip: 0x5d00 + source.len() as u64,
                            operation,
                            destination: (opcode == 0x8b).then_some(decoded_register),
                            source: (opcode == 0x89).then_some(decoded_register),
                            operand: decoded_operand,
                        };
                        assert!(
                            decoded_site.has_supported_profile(),
                            "profile rejected {source:02x?}: {decoded_operand:?}"
                        );
                        assert!(
                            decoded_site.matches_source_bytes(&source, operation),
                            "source rejected {source:02x?}: {decoded_operand:?}"
                        );
                        let direct_site = if opcode == 0x8b {
                            *compile_memory_load_block_v2(code, 0x5d00)
                                .unwrap_or_else(|error| {
                                    panic!("direct V2 rejected {source:02x?}: {error:?}")
                                })
                                .site()
                        } else {
                            *compile_memory_store_block_v2(code, 0x5d00)
                                .unwrap_or_else(|error| {
                                    panic!("direct V2 rejected {source:02x?}: {error:?}")
                                })
                                .site()
                        };
                        let shared_ir_site = if opcode == 0x8b {
                            *compile_ir_memory_load_block_v2(code, 0x5d00)
                                .unwrap_or_else(|error| {
                                    panic!("shared IR rejected {source:02x?}: {error:?}")
                                })
                                .site()
                        } else {
                            *compile_ir_memory_store_block_v2(code, 0x5d00)
                                .unwrap_or_else(|error| {
                                    panic!("shared IR rejected {source:02x?}: {error:?}")
                                })
                                .site()
                        };
                        for site in [&direct_site, &shared_ir_site] {
                            assert!(site.has_supported_profile());
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(
                                site.operand.base,
                                AddressBase::Register(Register::from_encoding(base).unwrap())
                            );
                            assert_eq!(site.operand.index, None);
                            assert_eq!(site.operand.scale, 1);
                        }
                        let mut state = od_isa::LongModeState {
                            rip: 0x5d00,
                            ..Default::default()
                        };
                        state.gpr[usize::from(base)] = 0x1234_5678;
                        assert_eq!(
                            shared_ir_site
                                .operand
                                .access_address_48(&state, shared_ir_site.next_rip()),
                            Ok(state.gpr[usize::from(base)]
                                .wrapping_add_signed(i64::from(displacement,)))
                        );
                        for visible in 1..source.len() {
                            let partial = CodeView::new(0x5d00, &source[..visible]).unwrap();
                            let result = if opcode == 0x8b {
                                compile_memory_load_block_v2(partial, 0x5d00).map(|_| ())
                            } else {
                                compile_memory_store_block_v2(partial, 0x5d00).map(|_| ())
                            };
                            assert!(result.is_err(), "partial source admitted: {source:02x?}");
                        }
                    }
                }
            }
        }

        for source in [
            vec![0x48, 0x8b, 0x04, 0x60], // no-index sentinel with non-unit scale
            vec![0x48, 0x8b, 0x04, 0x66], // no-index scale alias for RSI base
            vec![0x66, 0x48, 0x8b, 0x04, 0x20], // extra prefix
        ] {
            let code = CodeView::new(0x5d00, &source).unwrap();
            assert!(compile_memory_load_block_v2(code, 0x5d00).is_err());
            assert!(compile_ir_memory_load_block_v2(code, 0x5d00).is_err());
        }

        let first_disp = [0x48, 0x8b, 0x84, 0x26, 0, 0, 0, 1];
        let second_disp = [0x48, 0x8b, 0x84, 0x26, 0, 0, 0, 2];
        let first =
            compile_ir_memory_load_block_v2(CodeView::new(0x5d00, &first_disp).unwrap(), 0x5d00)
                .unwrap();
        let second =
            compile_ir_memory_load_block_v2(CodeView::new(0x5d00, &second_disp).unwrap(), 0x5d00)
                .unwrap();
        assert_ne!(first.source_bytes(), second.source_bytes());
        assert_ne!(
            first.site().operand.displacement,
            second.site().operand.displacement
        );
        let state = od_isa::LongModeState {
            gpr: [0, 0, 0, 0, 0, 0, 0x1000, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            first
                .site()
                .operand
                .access_address_48(&state, first.site().next_rip()),
            Ok(0x0100_1000)
        );
        assert_eq!(
            second
                .site()
                .operand
                .access_address_48(&state, second.site().next_rip()),
            Ok(0x0200_1000)
        );
    }

    #[test]
    fn rbp_base_no_index_sib_disp_has_exact_qword_profile() {
        let cases = [
            (1_u8, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        const ENTRY: u64 = 0x5e00;
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0_u8..=5 {
                for &(mode, displacements) in &cases {
                    for &displacement in displacements {
                        let mut source = vec![0x48, opcode, (mode << 6) | (data << 3) | 4, 0x25];
                        if mode == 1 {
                            source.push(displacement as u8);
                        } else {
                            source.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let code = CodeView::new(ENTRY, &source).unwrap();
                        let (direct_site, ir_site) = if opcode == 0x8b {
                            (
                                compile_memory_load_block_v2(code, ENTRY)
                                    .unwrap_or_else(|error| panic!("direct load rejected opcode={opcode:#x} data={data} mode={mode} disp={displacement}: {error:?}"))
                                    .site()
                                    .clone(),
                                compile_ir_memory_load_block_v2(code, ENTRY)
                                    .unwrap_or_else(|error| panic!("IR load rejected opcode={opcode:#x} data={data} mode={mode} disp={displacement}: {error:?}"))
                                    .site()
                                    .clone(),
                            )
                        } else {
                            (
                                compile_memory_store_block_v2(code, ENTRY)
                                    .unwrap()
                                    .site()
                                    .clone(),
                                compile_ir_memory_store_block_v2(code, ENTRY)
                                    .unwrap()
                                    .site()
                                    .clone(),
                            )
                        };
                        assert_eq!(direct_site, ir_site, "{source:02x?}");
                        for site in [&direct_site, &ir_site] {
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.next_rip(), ENTRY + source.len() as u64);
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.base, AddressBase::Register(Register::Rbp));
                            assert_eq!(site.operand.index, None);
                            assert_eq!(site.operand.scale, 1);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            let state = od_isa::LongModeState {
                                gpr: [
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0x0000_0001_8000_0000,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                ],
                                ..Default::default()
                            };
                            assert_eq!(
                                site.operand.access_address_48(&state, site.next_rip()),
                                Ok(state.gpr[5].wrapping_add_signed(i64::from(displacement))),
                                "{source:02x?}"
                            );
                        }

                        // The same decoded operation through direct ModR/M
                        // addressing remains a separate accepted encoding.
                        let direct_base_source = [
                            0x48,
                            opcode,
                            (mode << 6) | (data << 3) | 5,
                            source[4],
                            source.get(5).copied().unwrap_or(0),
                            source.get(6).copied().unwrap_or(0),
                            source.get(7).copied().unwrap_or(0),
                        ];
                        let direct_base_len = if mode == 1 { 4 } else { 7 };
                        let direct_base_code =
                            CodeView::new(ENTRY, &direct_base_source[..direct_base_len]).unwrap();
                        let direct_base_site = if opcode == 0x8b {
                            compile_ir_memory_load_block_v2(direct_base_code, ENTRY)
                                .unwrap()
                                .site()
                                .clone()
                        } else {
                            compile_ir_memory_store_block_v2(direct_base_code, ENTRY)
                                .unwrap()
                                .site()
                                .clone()
                        };
                        assert_eq!(direct_base_site.operand, ir_site.operand);
                        assert_eq!(direct_base_site.next_rip(), ENTRY + direct_base_len as u64);
                    }
                }
            }
        }

        // The identical SIB byte under mod00 remains the separate absolute
        // disp32 form; non-unit no-index scale is not admitted here.
        for opcode in [0x8b, 0x89] {
            let absolute = [0x48, opcode, 0x04, 0x25, 0, 0, 0, 0];
            let absolute_code = CodeView::new(ENTRY, &absolute).unwrap();
            if opcode == 0x8b {
                compile_ir_memory_load_block_v2(absolute_code, ENTRY).unwrap();
            } else {
                compile_ir_memory_store_block_v2(absolute_code, ENTRY).unwrap();
            }
            for source in [
                vec![0x48, opcode, 0x44, 0x65, 0],
                vec![0x48, opcode, 0x84, 0x65, 0, 0, 0, 0],
                vec![0x67, 0x48, opcode, 0x74, 0x25, 0], // unsupported RSI data register
            ] {
                let code = CodeView::new(ENTRY, &source).unwrap();
                let (direct, shared_ir) = if opcode == 0x8b {
                    (
                        compile_memory_load_block_v2(code, ENTRY).map(|_| ()),
                        compile_ir_memory_load_block_v2(code, ENTRY).map(|_| ()),
                    )
                } else {
                    (
                        compile_memory_store_block_v2(code, ENTRY).map(|_| ()),
                        compile_ir_memory_store_block_v2(code, ENTRY).map(|_| ()),
                    )
                };
                assert!(direct.is_err(), "direct admitted neighbor: {source:02x?}");
                assert!(shared_ir.is_err(), "IR admitted neighbor: {source:02x?}");
            }
            for (mode, length) in [(1_u8, 5_usize), (2, 8)] {
                let mut complete = vec![0x48, opcode, (mode << 6) | 4, 0x25];
                complete.resize(length, 0);
                for visible in 1..length {
                    let partial = CodeView::new(ENTRY, &complete[..visible]).unwrap();
                    let result = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(partial, ENTRY).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(partial, ENTRY).map(|_| ())
                    };
                    assert!(result.is_err(), "partial source admitted: {complete:02x?}");
                }
            }
        }
    }

    #[test]
    fn address32_rbp_base_no_index_sib_disp_has_exact_qword_profile() {
        let cases = [
            (1_u8, [i8::MIN as i32, -1, 0, i8::MAX as i32].as_slice()),
            (2, [i32::MIN, -1, 0, 1, i32::MAX].as_slice()),
        ];
        const ENTRY: u64 = 0x5f00;
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for data in 0_u8..=5 {
                for &(mode, displacements) in &cases {
                    for &displacement in displacements {
                        let mut source =
                            vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | 4, 0x25];
                        if mode == 1 {
                            source.push(displacement as u8);
                        } else {
                            source.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let code = CodeView::new(ENTRY, &source).unwrap();
                        let (direct_site, ir_site) = if opcode == 0x8b {
                            (
                                compile_memory_load_block_v2(code, ENTRY)
                                    .unwrap_or_else(|error| {
                                        panic!("direct load rejected {source:02x?}: {error:?}")
                                    })
                                    .site()
                                    .clone(),
                                compile_ir_memory_load_block_v2(code, ENTRY)
                                    .unwrap_or_else(|error| {
                                        panic!("IR load rejected {source:02x?}: {error:?}")
                                    })
                                    .site()
                                    .clone(),
                            )
                        } else {
                            (
                                compile_memory_store_block_v2(code, ENTRY)
                                    .unwrap_or_else(|error| {
                                        panic!("direct store rejected {source:02x?}: {error:?}")
                                    })
                                    .site()
                                    .clone(),
                                compile_ir_memory_store_block_v2(code, ENTRY)
                                    .unwrap_or_else(|error| {
                                        panic!("IR store rejected {source:02x?}: {error:?}")
                                    })
                                    .site()
                                    .clone(),
                            )
                        };
                        assert_eq!(direct_site, ir_site, "{source:02x?}");
                        for site in [&direct_site, &ir_site] {
                            assert!(site.has_supported_profile(), "{source:02x?}");
                            assert!(site.matches_source_bytes(&source, operation));
                            assert_eq!(site.next_rip(), ENTRY + source.len() as u64);
                            assert_eq!(site.operand.address_size, od_isa::OperandSize::Dword);
                            assert_eq!(site.operand.size, od_isa::OperandSize::Qword);
                            assert_eq!(site.operand.base, AddressBase::Register(Register::Rbp));
                            assert_eq!(site.operand.index, None);
                            assert_eq!(site.operand.scale, 1);
                            assert_eq!(site.operand.displacement, i64::from(displacement));
                            let state = od_isa::LongModeState {
                                gpr: [
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0xdead_beef_8000_0001,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                ],
                                ..Default::default()
                            };
                            let expected =
                                u64::from((state.gpr[5] as u32).wrapping_add(displacement as u32));
                            assert_eq!(
                                site.operand.access_address_48(&state, site.next_rip()),
                                Ok(expected)
                            );
                        }

                        let mut direct_bytes =
                            vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | 5];
                        if mode == 1 {
                            direct_bytes.push(displacement as u8);
                        } else {
                            direct_bytes.extend_from_slice(&displacement.to_le_bytes());
                        }
                        let direct_code = CodeView::new(ENTRY, &direct_bytes).unwrap();
                        let direct_operand = if opcode == 0x8b {
                            compile_ir_memory_load_block_v2(direct_code, ENTRY)
                                .unwrap()
                                .site()
                                .operand
                        } else {
                            compile_ir_memory_store_block_v2(direct_code, ENTRY)
                                .unwrap()
                                .site()
                                .operand
                        };
                        assert_eq!(direct_operand, ir_site.operand);
                        assert_eq!(direct_bytes.len() + 1, source.len());
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x74, 0x25, 0], // unsupported RSI data register
                vec![0x67, 0x48, opcode, 0x44, 0x65, 0], // non-unit scale/no index
                vec![0x48, 0x67, opcode, 0x44, 0x25, 0], // reordered prefix
            ] {
                let code = CodeView::new(ENTRY, &source).unwrap();
                let direct = if opcode == 0x8b {
                    compile_memory_load_block_v2(code, ENTRY).map(|_| ())
                } else {
                    compile_memory_store_block_v2(code, ENTRY).map(|_| ())
                };
                let shared_ir = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, ENTRY).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, ENTRY).map(|_| ())
                };
                assert!(direct.is_err(), "direct admitted neighbor {source:02x?}");
                assert!(shared_ir.is_err(), "IR admitted neighbor {source:02x?}");
            }
            for (mode, length) in [(1_u8, 6_usize), (2, 9)] {
                let mut complete = vec![0x67, 0x48, opcode, (mode << 6) | 4, 0x25];
                complete.resize(length, 0);
                for visible in 1..length {
                    let partial = CodeView::new(ENTRY, &complete[..visible]).unwrap();
                    let direct = if opcode == 0x8b {
                        compile_memory_load_block_v2(partial, ENTRY).map(|_| ())
                    } else {
                        compile_memory_store_block_v2(partial, ENTRY).map(|_| ())
                    };
                    let shared_ir = if opcode == 0x8b {
                        compile_ir_memory_load_block_v2(partial, ENTRY).map(|_| ())
                    } else {
                        compile_ir_memory_store_block_v2(partial, ENTRY).map(|_| ())
                    };
                    assert!(direct.is_err(), "direct admitted partial {complete:02x?}");
                    assert!(shared_ir.is_err(), "IR admitted partial {complete:02x?}");
                }
            }
        }
    }

    #[test]
    fn address32_rsi_rdi_base_no_index_sib_has_exact_qword_profile() {
        const ENTRY: u64 = 0x5f80;
        let edges = [
            (0_u8, 0_i32),
            (1, i8::MIN as i32),
            (1, -1),
            (1, 0),
            (1, 2),
            (1, i8::MAX as i32),
            (2, i32::MIN),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, i32::MAX),
        ];
        for (opcode, operation) in [
            (0x8b, MemorySiteOperationV2::Load),
            (0x89, MemorySiteOperationV2::Store),
        ] {
            for (base, sib) in [(Register::Rsi, 0x26), (Register::Rdi, 0x27)] {
                for data in 0_u8..=5 {
                    for &(mode, displacement) in &edges {
                        let mut source =
                            vec![0x67, 0x48, opcode, (mode << 6) | (data << 3) | 4, sib];
                        match mode {
                            0 => {}
                            1 => source.push(displacement as u8),
                            2 => source.extend_from_slice(&displacement.to_le_bytes()),
                            _ => unreachable!(),
                        }
                        let code = CodeView::new(ENTRY, &source).unwrap();
                        let (direct_site, ir_site) = if opcode == 0x8b {
                            let direct = compile_memory_load_block_v2(code, ENTRY).unwrap();
                            let ir = compile_ir_memory_load_block_v2(code, ENTRY).unwrap_or_else(
                                |error| panic!("IR load rejected {source:02x?}: {error:?}"),
                            );
                            (direct.site().clone(), ir.site().clone())
                        } else {
                            let direct = compile_memory_store_block_v2(code, ENTRY).unwrap();
                            let ir = compile_ir_memory_store_block_v2(code, ENTRY).unwrap_or_else(
                                |error| panic!("IR store rejected {source:02x?}: {error:?}"),
                            );
                            (direct.site().clone(), ir.site().clone())
                        };
                        assert_eq!(direct_site, ir_site, "{source:02x?}");
                        assert!(direct_site.has_supported_profile(), "{source:02x?}");
                        assert!(direct_site.matches_source_bytes(&source, operation));
                        assert_eq!(direct_site.next_rip(), ENTRY + source.len() as u64);
                        assert_eq!(direct_site.operand.address_size, od_isa::OperandSize::Dword);
                        assert_eq!(direct_site.operand.size, od_isa::OperandSize::Qword);
                        assert_eq!(direct_site.operand.base, AddressBase::Register(base));
                        assert_eq!(direct_site.operand.index, None);
                        assert_eq!(direct_site.operand.scale, 1);
                        assert_eq!(direct_site.operand.displacement, i64::from(displacement));

                        let base_index = base as usize;
                        let state = od_isa::LongModeState {
                            gpr: {
                                let mut gpr = [0_u64; 16];
                                gpr[base_index] = 0xdead_beef_ffff_ffff;
                                gpr
                            },
                            ..Default::default()
                        };
                        let expected = u64::from(
                            (state.gpr[base_index] as u32).wrapping_add(displacement as u32),
                        );
                        assert_eq!(
                            direct_site
                                .operand
                                .access_address_48(&state, direct_site.next_rip()),
                            Ok(expected)
                        );

                        // The equivalent address32 direct-base form omits the SIB byte.
                        let mut direct_bytes = vec![
                            0x67,
                            0x48,
                            opcode,
                            (mode << 6) | (data << 3) | if base == Register::Rsi { 6 } else { 7 },
                        ];
                        match mode {
                            0 => {}
                            1 => direct_bytes.push(displacement as u8),
                            2 => direct_bytes.extend_from_slice(&displacement.to_le_bytes()),
                            _ => unreachable!(),
                        }
                        let direct_code = CodeView::new(ENTRY, &direct_bytes).unwrap();
                        let direct_operand = if opcode == 0x8b {
                            compile_ir_memory_load_block_v2(direct_code, ENTRY)
                                .unwrap()
                                .site()
                                .operand
                        } else {
                            compile_ir_memory_store_block_v2(direct_code, ENTRY)
                                .unwrap()
                                .site()
                                .operand
                        };
                        assert_eq!(direct_operand, direct_site.operand, "{source:02x?}");
                        assert_eq!(direct_bytes.len() + 1, source.len());

                        for visible in 1..source.len() {
                            let partial = CodeView::new(ENTRY, &source[..visible]).unwrap();
                            let (direct_result, ir_result) = if opcode == 0x8b {
                                (
                                    compile_memory_load_block_v2(partial, ENTRY).map(|_| ()),
                                    compile_ir_memory_load_block_v2(partial, ENTRY).map(|_| ()),
                                )
                            } else {
                                (
                                    compile_memory_store_block_v2(partial, ENTRY).map(|_| ()),
                                    compile_ir_memory_store_block_v2(partial, ENTRY).map(|_| ()),
                                )
                            };
                            assert!(
                                direct_result.is_err(),
                                "direct admitted partial {source:02x?}"
                            );
                            assert!(ir_result.is_err(), "IR admitted partial {source:02x?}");
                        }
                    }
                }
            }
        }

        for opcode in [0x8b, 0x89] {
            for source in [
                vec![0x67, 0x48, opcode, 0x74, 0x26, 0], // data register RSI
                vec![0x66, 0x67, 0x48, opcode, 0x04, 0x26], // extra operand-size prefix
                vec![0x67, 0x48, 0x66, opcode, 0x04, 0x26], // extra prefix after REX
                vec![0x48, 0x67, opcode, 0x04, 0x26],    // reordered prefix
            ] {
                let code = CodeView::new(ENTRY, &source).unwrap();
                let direct = if opcode == 0x8b {
                    compile_memory_load_block_v2(code, ENTRY).map(|_| ())
                } else {
                    compile_memory_store_block_v2(code, ENTRY).map(|_| ())
                };
                let ir = if opcode == 0x8b {
                    compile_ir_memory_load_block_v2(code, ENTRY).map(|_| ())
                } else {
                    compile_ir_memory_store_block_v2(code, ENTRY).map(|_| ())
                };
                assert!(direct.is_err(), "direct admitted neighbor {source:02x?}");
                assert!(ir.is_err(), "IR admitted neighbor {source:02x?}");
            }
        }
    }
}
