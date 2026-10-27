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
    source_bytes: [u8; 8],
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
    use od_isa::Register;
    use od_isa::long_decode::AddressBase;

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
            vec![0x67, 0x48, 0x8b, 0x00], // address-size override
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
}
