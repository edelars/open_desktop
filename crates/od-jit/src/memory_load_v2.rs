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
    LONG_MODE_GPR_OFFSET, LONG_MODE_RIP_OFFSET, MemorySiteOperationV2, MemorySiteV2,
    RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET, RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET,
    RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET, RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET,
    RUNTIME_CONTEXT_STOP_RIP_ENABLED_OFFSET, RUNTIME_CONTEXT_STOP_RIP_OFFSET,
    RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET, RUNTIME_CONTEXT_V3_CODE_DIRTY_OFFSET,
    RuntimeExitV3, memory_site_helper_v2,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryLoadCompileErrorV2 {
    EntryOutsideCode,
    Decode,
    UnsupportedForm,
    Encoding(EncodeError),
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
    source_bytes: [u8; 8],
    source_len: u8,
    compile_flavor: crate::translation::CompileFlavor,
}

/// Compiler-issued load artifact. Arbitrary code bytes cannot be promoted to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryLoadBlockV2 {
    artifact: MemoryArtifactV2,
}

/// Compiler-issued checked load lowered from the shared typed IR/allocator
/// path. Its entry and publication contract are distinct from direct V2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrMemoryLoadBlockV2 {
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

/// Compiler-issued store artifact, kept distinct from load publication/API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryStoreBlockV2 {
    artifact: MemoryArtifactV2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveCodeArtifactV3 {
    code: EncodedCode,
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
    let code = emit_active_code_block_v3(&site, entry_rip, prefix_value, tail_value)?;
    Ok(ActiveCodeBlockV3 {
        artifact: ActiveCodeArtifactV3 {
            code,
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
) -> Result<EncodedCode, MemoryLoadCompileErrorV2> {
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
    emit_checked_memory_site_call_v2(&mut encoder, site.instruction_index())?;
    encoder.cbnz_w(WReg::new(0)?, helper_failure)?;
    emit_imm64(&mut encoder, 9, site.next_rip())?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        HostAddressBase::Reg(xr(20)?),
        LONG_MODE_RIP_OFFSET as u32,
    )?;
    decrement_remaining_and_retire(&mut encoder, 2)?;
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
    encoder.ldarb_w(WReg::new(9)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(9)?, stop_after_tail)?;
    encoder.b(complete)?;

    encoder.bind(code_invalidated)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::CodeInvalidated as u16)?;
    encoder.bind(helper_failure)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::MemoryOperation as u16)?;
    encoder.bind(stop_address)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::StopAddress as u16)?;
    encoder.bind(stop_requested)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::StopRequested as u16)?;
    encoder.bind(budget)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::BudgetExhausted as u16)?;
    encoder.bind(stop_after_tail)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::StopRequested as u16)?;
    encoder.bind(complete)?;
    emit_return_v3(&mut encoder, RuntimeExitV3::Complete as u16)?;
    encoder.finish().map_err(MemoryLoadCompileErrorV2::Encoding)
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

            pub const fn source_bytes_storage(&self) -> ([u8; 8], u8) {
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

/// Compile the exact checked-memory V2 profile from decoded guest bytes.
///
/// The decoder remains the source of operand semantics. The raw-byte checks
/// narrow admission to the explicitly reviewed form; they do not decode it.
pub fn compile_memory_load_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<MemoryLoadBlockV2, MemoryLoadCompileErrorV2> {
    Ok(MemoryLoadBlockV2 {
        artifact: compile_memory_operation_v2(code, entry_rip, MemorySiteOperationV2::Load)?,
    })
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
    let mut source_bytes = [0; 8];
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

/// Compile exactly one checked qword store through BasicBlock → typed IR →
/// allocator → runtime emitter. The artifact has its own entry/API type.
pub fn compile_ir_memory_store_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryStoreBlockV2, MemoryLoadCompileErrorV2> {
    let (site, bytes) = checked_memory_metadata_v2(code, entry_rip, MemorySiteOperationV2::Store)?;
    if !(crate::block::exact_direct_qword_memory_bytes(code, entry_rip, bytes.len() as u8, 0x89)
        || crate::block::exact_qword_address32_memory_bytes(
            code,
            entry_rip,
            bytes.len() as u8,
            0x89,
        ))
        || !site.has_supported_profile()
        || !site.matches_source_bytes(bytes, MemorySiteOperationV2::Store)
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let emitted = crate::block_compile::compile_checked_store_block_v2(code, entry_rip)
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let mut source_bytes = [0; 8];
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

/// Compile one of the two exact reviewed qword-memory pairs.
pub fn compile_ir_memory_two_site_block_v2(
    code: CodeView<'_>,
    entry_rip: u64,
) -> Result<IrMemoryTwoSiteBlockV2, MemoryLoadCompileErrorV2> {
    if entry_rip != code.base_rip()
        || code.len() != 6
        || !matches!(
            code.bytes(),
            [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b] | [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02]
        )
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let second_rip = entry_rip
        .checked_add(3)
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let store_view = CodeView::new(entry_rip, &code.bytes()[..3])
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let load_view = CodeView::new(second_rip, &code.bytes()[3..])
        .map_err(|_| MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let load_then_store = code.bytes()[..3] == [0x48, 0x8b, 0x03];
    let (first_operation, second_operation) = if load_then_store {
        (MemorySiteOperationV2::Load, MemorySiteOperationV2::Store)
    } else {
        (MemorySiteOperationV2::Store, MemorySiteOperationV2::Load)
    };
    let (mut first, _) = checked_memory_metadata_v2(store_view, entry_rip, first_operation)?;
    let (mut second, _) = checked_memory_metadata_v2(load_view, second_rip, second_operation)?;
    let operands_match = if load_then_store {
        first.destination() == Some(0)
            && first.operand().base
                == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && second.source() == Some(0)
            && second.operand().base
                == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rdx)
    } else {
        first.source() == Some(0)
            && first.operand().base
                == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && second.destination() == Some(1)
            && second.operand().base
                == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
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
        sequence: if load_then_store {
            crate::runtime::MemoryPairSequenceV2::LoadThenStore
        } else {
            crate::runtime::MemoryPairSequenceV2::StoreThenLoad
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
    if !(3..=8).contains(&bytes.len()) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let mut memory = CodeMemory::full_code(code);
    let decoded = long_decode::decode(&mut memory, entry_rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)?;
    if decoded.length as usize != bytes.len()
        || !crate::block::is_checked_qword_load_form(decoded)
        || !(crate::block::exact_direct_qword_memory_bytes(code, entry_rip, decoded.length, 0x8b)
            || crate::block::exact_qword_address32_memory_bytes(
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
    Ok(MemoryStoreBlockV2 {
        artifact: compile_memory_operation_v2(code, entry_rip, MemorySiteOperationV2::Store)?,
    })
}

fn compile_memory_operation_v2(
    code: CodeView<'_>,
    entry_rip: u64,
    access: MemorySiteOperationV2,
) -> Result<MemoryArtifactV2, MemoryLoadCompileErrorV2> {
    let (site, bytes) = checked_memory_metadata_v2(code, entry_rip, access)?;
    let code = emit_memory_operation_v2(&site).map_err(MemoryLoadCompileErrorV2::Encoding)?;
    let compile_flavor = match access {
        MemorySiteOperationV2::Load => crate::translation::CompileFlavor::S3_19bMemoryLoadV2,
        MemorySiteOperationV2::Store => crate::translation::CompileFlavor::S3_20bMemoryStoreV2,
    };
    let mut source_bytes = [0; 8];
    source_bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(MemoryArtifactV2 {
        code,
        site,
        entry_rip,
        source_bytes,
        source_len: bytes.len() as u8,
        compile_flavor,
    })
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
    if !(3..=8).contains(&instruction_bytes_len) {
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

fn emit_memory_operation_v2(site: &MemorySiteV2) -> Result<EncodedCode, EncodeError> {
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

    emit_checked_memory_site_call_v2(&mut encoder, site.instruction_index)?;
    encoder.cbnz_w(WReg::new(0)?, helper_failure)?;

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
    encoder.ldarb_w(WReg::new(10)?, HostAddressBase::Reg(xr(21)?));
    encoder.cbnz_w(WReg::new(10)?, stop_after_commit)?;
    emit_return(&mut encoder, 0)?;

    encoder.bind(budget_exit)?;
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
    emit_return(&mut encoder, 3)?;

    encoder.bind(stop_address_exit)?;
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
    encoder.finish()
}

/// Shared low-level call primitive. It only transports the V2 context/site to
/// the checked helper and returns its raw helper status in w0; callers retain
/// ownership of guards, architectural commits, retirement and exits.
fn emit_checked_memory_site_call_v2(
    encoder: &mut Encoder,
    site_index: u32,
) -> Result<(), EncodeError> {
    encoder.mov_x(xr(0)?, XRegOrZero::Reg(xr(19)?));
    encoder.movz_x(xr(1)?, site_index as u16, 0)?;
    let helper = memory_site_helper_v2 as *const () as usize as u64;
    encoder.movz_x(xr(16)?, helper as u16, 0)?;
    encoder.movk_x(xr(16)?, (helper >> 16) as u16, 1)?;
    encoder.movk_x(xr(16)?, (helper >> 32) as u16, 2)?;
    encoder.movk_x(xr(16)?, (helper >> 48) as u16, 3)?;
    encoder.blr(xr(16)?);
    Ok(())
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
    use od_isa::Register;
    use od_isa::long_decode::AddressBase;

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
                vec![0x48, opcode, 0x04],                   // truncated before SIB
                vec![0x48, opcode, 0x04, 0x20],             // no-index
                vec![0x48, opcode, 0x04, 0x05],             // no-base
                vec![0x48, opcode, 0x04, 0x24],             // RSP index
                vec![0x48, opcode, 0x04, 0x28],             // RSP base
                vec![0x48, opcode, 0x44, 0x18],             // displaced SIB
                vec![0x48, opcode, 0x84, 0x18, 0, 0, 0, 0], // disp32 SIB
                vec![0x4a, opcode, 0x04, 0x18],             // extended index
                vec![0x49, opcode, 0x04, 0x18],             // extended base
                vec![0x48, opcode, 0x04, 0x18, 0x90],       // trailing byte
                vec![0x67, 0x48, opcode, 0x04, 0x18],       // address override
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
    fn ir_checked_direct_displacements_reject_prefix_register_and_trailing_forms() {
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x48, opcode, 0xc3],             // mod=11
                vec![0x48, opcode, 0x44, 0x18],       // SIB with disp8
                vec![0x67, 0x48, opcode, 0x43, 0x01], // address override
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
    fn ir_rip_relative_singletons_reject_wrong_register_prefix_and_trailing_bytes() {
        for opcode in [0x8b, 0x89] {
            for bytes in [
                vec![0x48, opcode, 0x25, 0, 0, 0, 0], // data register RSP
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
            vec![0x48, 0x8b, 0x20],
            vec![0x48, 0x8b, 0xc0],       // register-register mod=11
            vec![0x48, 0x8b, 0x04],       // truncated SIB
            vec![0x48, 0x8b, 0x04, 0x24], // RSP index is unsupported
            vec![0x48, 0x8b, 0x04, 0x28], // RBP base requires disp32
            vec![0x48, 0x8b, 0x04, 0x20], // no-index SIB
            vec![0x48, 0x8b, 0x04, 0x05], // no-base SIB requires disp32
            vec![0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0], // no-base SIB
            vec![0x48, 0x8b, 0x44, 0x20, 1], // no-index SIB with disp8
            vec![0x48, 0x8b, 0x84, 0x20, 1, 0, 0, 0], // no-index SIB with disp32
            vec![0x48, 0x8b, 0x44, 0x00], // mod=01 displacement
            vec![0x48, 0x8b, 0x40],       // truncated disp8
            vec![0x48, 0x8b, 0x80, 0, 0, 0], // truncated disp32
            vec![0x48, 0x8b, 0x05],       // truncated RIP-relative disp32
            vec![0x48, 0x8b, 0x05, 0, 0, 0, 0, 0x90], // RIP-relative tail
            vec![0x48, 0x8b, 0x45, 0],    // mod01 RBP base is unsupported
            vec![0x48, 0x8b, 0x85, 0, 0, 0, 0], // mod10 RBP base is unsupported
            vec![0x4a, 0x8b, 0x04, 0x00], // REX.X extended index
            vec![0x49, 0x8b, 0x04, 0x00], // REX.B extended base
            vec![0x67, 0x48, 0x8b, 0x00, 0x90], // address-size override tail
            vec![0x67, 0x48, 0x8b],       // truncated address-size override form
            vec![0x67, 0x48, 0x8b, 0x40], // address-size override mod01
            vec![0x67, 0x48, 0x8b, 0x80], // address-size override mod10
            vec![0x67, 0x48, 0x8b, 0xc0], // address-size override mod11
            vec![0x67, 0x48, 0x8b, 0x04], // address-size override SIB
            vec![0x67, 0x48, 0x8b, 0x05, 0, 0, 0, 0], // no-base disp32
            vec![0x67, 0x48, 0x8b, 0x45, 0], // RBP with displacement
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
                vec![0x67, 0x48, opcode, 0xc0],       // register form
                vec![0x67, 0x48, opcode, 0x04, 0x18], // SIB
                vec![0x67, 0x48, opcode, 0x40, 0],    // disp8
                vec![0x67, 0x48, opcode, 0x80, 0, 0, 0, 0], // disp32
                vec![0x67, 0x4c, opcode, 0],          // extended data/base
                vec![0x48, 0x67, opcode, 0],          // wrong prefix order
                vec![0x67, 0x48, opcode, 0, 0x90],    // trailing byte
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
                vec![0x67, 0x48, opcode, 0x04, 0],                // SIB form
                vec![0x67, 0x48, opcode, 0x40 | (4 << 3), 0],     // unsupported base
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
            for register in 0..4_u8 {
                for displacement in [i32::MIN, -8, -7, -1, 0, 1, 8, i32::MAX] {
                    let mut instruction = vec![0x48, opcode, (register << 3) | 5];
                    instruction.extend_from_slice(&displacement.to_le_bytes());
                    let mut view_bytes = vec![0x90, 0x90];
                    view_bytes.extend_from_slice(&instruction);
                    let code = CodeView::new(0x6000, &view_bytes).unwrap();
                    let entry_rip = 0x6002;
                    let artifact = if is_load {
                        compile_memory_load_block_v2(code, entry_rip)
                            .unwrap()
                            .artifact
                    } else {
                        compile_memory_store_block_v2(code, entry_rip)
                            .unwrap()
                            .artifact
                    };
                    assert_eq!(artifact.source_len, 7);
                    assert_eq!(&artifact.source_bytes[..7], instruction.as_slice());
                    assert_eq!(artifact.site.guest_rip(), entry_rip);
                    assert_eq!(artifact.site.next_rip(), entry_rip + 7);
                    assert_eq!(artifact.site.operand.base, AddressBase::NextRip);
                    assert_eq!(artifact.site.operand.index, None);
                    assert_eq!(artifact.site.operand.scale, 1);
                    assert_eq!(artifact.site.operand.displacement, i64::from(displacement));
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
            vec![0x48, 0x89, 0x18, 0x90], // trailing instruction
            vec![0x40, 0x48, 0x89, 0x18], // extra prefix
            vec![0x48, 0x89, 0x1c],       // SIB
            vec![0x48, 0x89, 0x58],       // displacement
            vec![0x48, 0x89, 0x20],       // out-of-profile source register
        ] {
            let code = CodeView::new(0x5000, &bytes).unwrap();
            assert!(matches!(
                compile_memory_store_block_v2(code, 0x5000),
                Err(MemoryLoadCompileErrorV2::UnsupportedForm)
                    | Err(MemoryLoadCompileErrorV2::Decode)
            ));
        }
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
}
