//! Verified compiler path for the first V2 checked-memory block profile.
//!
//! This deliberately accepts only one decoded `MOV r64, [base]` instruction
//! in the exact `48 8B /r` encoding profile. It does not extend V1 admission.

use od_isa::Register;
use od_isa::long_decode::{self, AddressBase, Operand, Operation};
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
    RUNTIME_CONTEXT_V2_STAGED_LOAD_VALUE_OFFSET, memory_site_helper_v2,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryLoadCompileErrorV2 {
    EntryOutsideCode,
    Decode,
    UnsupportedForm,
    Encoding(EncodeError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MemoryArtifactV2 {
    code: EncodedCode,
    site: MemorySiteV2,
    entry_rip: u64,
    source_bytes: [u8; 4],
    source_len: u8,
    compile_flavor: crate::translation::CompileFlavor,
}

/// Compiler-issued load artifact. Arbitrary code bytes cannot be promoted to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryLoadBlockV2 {
    artifact: MemoryArtifactV2,
}

/// Compiler-issued store artifact, kept distinct from load publication/API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryStoreBlockV2 {
    artifact: MemoryArtifactV2,
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

            pub const fn source_bytes_storage(&self) -> ([u8; 4], u8) {
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
    let offset = entry_rip
        .checked_sub(code.base_rip())
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < code.len())
        .ok_or(MemoryLoadCompileErrorV2::EntryOutsideCode)?;
    let instruction_bytes_len = code.len() - offset;
    if !(3..=4).contains(&instruction_bytes_len) {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let mut memory = CodeMemory::full_code(code);
    let decoded = long_decode::decode(&mut memory, entry_rip, AccessContext::default())
        .map_err(|_| MemoryLoadCompileErrorV2::Decode)?;
    let bytes = code
        .bytes()
        .get(offset..offset.saturating_add(instruction_bytes_len))
        .ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?;
    let opcode = match access {
        MemorySiteOperationV2::Load => 0x8b,
        MemorySiteOperationV2::Store => 0x89,
    };
    if bytes.len() < 3 || bytes[0] != 0x48 || bytes[1] != opcode {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
    let modrm = bytes[2];
    if decoded.length as usize != instruction_bytes_len || modrm >> 6 != 0 || (modrm >> 3) & 7 >= 4
    {
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
    let decoded_base = match operand.base {
        AddressBase::Register(base) => base,
        AddressBase::None | AddressBase::NextRip => {
            return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
        }
    };
    let raw_rm = modrm & 7;
    let raw_base = if raw_rm == 4 {
        if bytes.len() != 4 || decoded.length != 4 {
            return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
        }
        let sib = bytes[3];
        let base = sib & 7;
        let index = (sib >> 3) & 7;
        let scale = 1_u8 << (sib >> 6);
        if base >= 4 || index >= 4 {
            return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
        }
        if operand.index != Register::from_encoding(index)
            || operand.scale != scale
            || Register::from_encoding(base) != Some(decoded_base)
        {
            return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
        }
        base
    } else {
        if bytes.len() != 3 || raw_rm >= 4 || operand.index.is_some() || operand.scale != 1 {
            return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
        }
        raw_rm
    };
    if register as u8 >= 4
        || !matches!(
            decoded_base,
            Register::Rax | Register::Rcx | Register::Rdx | Register::Rbx
        )
        || operand.index.is_some_and(|index| {
            !matches!(
                index,
                Register::Rax | Register::Rcx | Register::Rdx | Register::Rbx
            )
        })
        || operand.displacement != 0
        || operand.address_size != od_isa::OperandSize::Qword
        || operand.size != od_isa::OperandSize::Qword
        || decoded_base
            != Register::from_encoding(raw_base).ok_or(MemoryLoadCompileErrorV2::UnsupportedForm)?
        || !matches!(operand.scale, 1 | 2 | 4 | 8)
        || register as u8 != ((modrm >> 3) & 7)
    {
        return Err(MemoryLoadCompileErrorV2::UnsupportedForm);
    }
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
    let code = emit_memory_operation_v2(&site).map_err(MemoryLoadCompileErrorV2::Encoding)?;
    let compile_flavor = match access {
        MemorySiteOperationV2::Load => crate::translation::CompileFlavor::S3_19bMemoryLoadV2,
        MemorySiteOperationV2::Store => crate::translation::CompileFlavor::S3_20bMemoryStoreV2,
    };
    Ok(MemoryArtifactV2 {
        code,
        site,
        entry_rip,
        source_bytes: [
            0x48,
            opcode,
            modrm,
            if bytes.len() == 4 { bytes[3] } else { 0 },
        ],
        source_len: bytes.len() as u8,
        compile_flavor,
    })
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

    encoder.mov_x(xr(0)?, XRegOrZero::Reg(xr(19)?));
    encoder.movz_x(xr(1)?, site.instruction_index as u16, 0)?;
    let helper = memory_site_helper_v2 as *const () as usize as u64;
    encoder.movz_x(xr(16)?, helper as u16, 0)?;
    encoder.movk_x(xr(16)?, (helper >> 16) as u16, 1)?;
    encoder.movk_x(xr(16)?, (helper >> 32) as u16, 2)?;
    encoder.movk_x(xr(16)?, (helper >> 48) as u16, 3)?;
    encoder.blr(xr(16)?);
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
            vec![0x48, 0x8b, 0x40, 0x00],
            vec![0x48, 0x8b, 0x20],
            vec![0x48, 0x8b, 0x04],             // truncated SIB
            vec![0x48, 0x8b, 0x04, 0x24],       // RSP index is unsupported
            vec![0x48, 0x8b, 0x04, 0x28],       // RBP base requires disp32
            vec![0x48, 0x8b, 0x04, 0x20],       // no-index SIB
            vec![0x48, 0x8b, 0x04, 0x05],       // no-base SIB requires disp32
            vec![0x48, 0x8b, 0x44, 0x00],       // mod=01 displacement
            vec![0x4a, 0x8b, 0x04, 0x00],       // REX.X extended index
            vec![0x49, 0x8b, 0x04, 0x00],       // REX.B extended base
            vec![0x48, 0x8b, 0x04, 0x00, 0x90], // ignored tail
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
}
