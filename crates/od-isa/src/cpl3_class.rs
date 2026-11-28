//! ADR-0023 slice C1 (S7-32): the CPL3 classification of the operations of the CPL0 set and the
//! decision function of the privilege layer of its step.
//!
//! [`cpl3_class`] is exhaustive over `long_decode::Operation` (a `match` without `_`): a new operation
//! does not compile without its class. [`cpl3_gate`] turns a class, CR4, CPL and IOPL into the
//! decision of the layer (ADR-0023, decision 1 and "Порядок проверок"; Intel SDM 325462-093US). The
//! layer runs only in the set at CPL > 0, after the #UD conditions of the decode list
//! (`execute::cpl3_decode_list_fault`) and before any memory access of the instruction.

use crate::execute::{CR4_PCE, CR4_TSD, CR4_UMIP};
use crate::long_decode::Operation;

/// The class of an operation at CPL3 (ADR-0023, decision 1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl3Class {
    /// The semantics do not depend on CPL; explicit accesses use the privilege of the context.
    Neutral,
    /// CPL > 0 is #GP(0) after the decode list and before any effect.
    Privileged,
    /// The CR4 bit of the gate decides (Vol. 2B, the instruction pages).
    Cr4Gated(Cr4Gate),
    /// IOPL decides (Vol. 1 §20.5.1-20.5.2).
    IoplSensitive(IoplKind),
    /// An executor with its own CPL3 rules.
    CplSpecific,
    /// An honest `Unsupported` at CPL3 with nothing changed.
    NotAtCpl3,
}

/// The CR4 bit of a [`Cpl3Class::Cr4Gated`] operation. Only bits of operations the decoder knows:
/// FSGSBASE, OSXSAVE, PKE and CET have no decoded operation (ADR-0023 slice C2c); the CR4 policy of
/// the set keeps them clear (`const _` checks in `execute.rs`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cr4Gate {
    /// CR4.TSD (bit 2): `RDTSC` is #GP(0) at CPL > 0 when set.
    Tsd,
    /// CR4.PCE (bit 8): `RDPMC` is #GP(0) at CPL > 0 when clear.
    Pce,
    /// CR4.UMIP (bit 11): `SGDT`, `SIDT`, `SLDT`, `STR` are #GP(0) at CPL > 0 when set.
    Umip,
}

/// The IOPL rule of a [`Cpl3Class::IoplSensitive`] operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoplKind {
    /// `CLI`, `STI`: #GP(0) if IOPL < CPL.
    InterruptFlag,
    /// `IN`, `OUT`: at CPL > IOPL the I/O permission bit map of the TSS decides.
    PortIo,
    /// `POPF`: IF only if CPL <= IOPL, IOPL only at CPL0; no #GP.
    Popf,
}

/// The decision of the privilege layer for one instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cpl3Gate {
    /// The step executes the operation as it does at CPL0.
    Execute,
    /// #GP(0) before any memory access.
    Gp0,
    /// The executor of the operation applies its CPL rules.
    ByExecutor,
    /// `Unsupported` with nothing changed.
    NotModeled,
}

/// The CPL3 class of `operation` (ADR-0023, decision 1; the table of plan C1/C2a, appendix A).
pub const fn cpl3_class(operation: &Operation) -> Cpl3Class {
    match operation {
        // SDM 325462-093US Vol. 3A §6.9 and §2.8 Table 2-3 (HLT, WBINVD, RDMSR, WRMSR, LGDT, LIDT, LLDT, LTR,
        // MOV CRn, MOV DRn, INVLPG); Vol. 2B "SWAPGS", "SYSRET", "SYSEXIT": #GP(0) if CPL > 0. The narrow forms
        // (`WriteCr0Rdi`, `ReadCr3Into`, `WriteCr4From`, ...) are classed like their general forms (ADR-0023,
        // decision 1).
        Operation::Hlt
        | Operation::Wbinvd
        | Operation::ReadMsr
        | Operation::WriteMsr
        | Operation::Sysexit
        | Operation::Sysret
        | Operation::Swapgs
        | Operation::LoadGdtr { .. }
        | Operation::LoadIdtr { .. }
        | Operation::LoadLdtMemory { .. }
        | Operation::ReadControl { .. }
        | Operation::WriteControl { .. }
        | Operation::ReadDebugRegister { .. }
        | Operation::LoadTr { .. }
        | Operation::LoadLdt { .. }
        | Operation::WriteDebugRegister { .. }
        | Operation::ReadCr0
        | Operation::WriteCr0
        | Operation::WriteCr0Rdi
        | Operation::ReadCr4
        | Operation::WriteCr4
        | Operation::WriteCr3
        | Operation::ReadCr3
        | Operation::ReadCr2
        | Operation::ReadCr3Into(..)
        | Operation::ReadCr2Into(..)
        | Operation::ReadCr4Into(..)
        | Operation::WriteCr4From(..)
        | Operation::WriteCr3From(..)
        | Operation::Invlpg => Cpl3Class::Privileged,
        // Vol. 2B "RDTSC": #GP(0) if CR4.TSD = 1 and CPL > 0.
        Operation::Rdtsc => Cpl3Class::Cr4Gated(Cr4Gate::Tsd),
        // Vol. 2B "RDPMC": #GP(0) if CR4.PCE = 0 and CPL > 0.
        Operation::Rdpmc => Cpl3Class::Cr4Gated(Cr4Gate::Pce),
        // Vol. 2B "SGDT", "SIDT", "SLDT", "STR": #GP(0) if CR4.UMIP = 1 and CPL > 0.
        Operation::StoreGdtr { .. }
        | Operation::StoreIdtr { .. }
        | Operation::StoreSystemSelector { .. } => Cpl3Class::Cr4Gated(Cr4Gate::Umip),
        // Vol. 2A "CLI", Vol. 2B "STI": #GP(0) if IOPL < CPL (CR4.PVI is outside the policy, `CliPreconditionFailed`).
        Operation::Cli
        | Operation::Sti => Cpl3Class::IoplSensitive(IoplKind::InterruptFlag),
        // Vol. 1 §20.5.1-20.5.2, Vol. 2A "IN", Vol. 2B "OUT": at CPL > IOPL the I/O permission bit map of the TSS decides.
        Operation::InAlDx
        | Operation::OutDxAl
        | Operation::InAlImm8 { .. }
        | Operation::OutImm8Al { .. }
        | Operation::InDx { .. }
        | Operation::OutDx { .. } => Cpl3Class::IoplSensitive(IoplKind::PortIo),
        // Vol. 2B "POPF": IF changes only if CPL <= IOPL, IOPL only at CPL0; no #GP.
        Operation::PopFlags => Cpl3Class::IoplSensitive(IoplKind::Popf),
        // Own executors with the CPL3 rules (ADR-0023, decision 1): INT n and INT3 (gate DPL), SYSCALL,
        // IRETQ (Vol. 2A "IRET": a return to the same level), VERR/VERW/LAR/LSL (descriptor visibility,
        // DPL >= CPL and DPL >= RPL) and the segment-register loads (Vol. 2A "MOV", "POP": RPL/DPL against CPL).
        Operation::Syscall
        | Operation::VerwGdt
        | Operation::VerwMem { .. }
        | Operation::MovToSreg { .. }
        | Operation::SoftwareInterrupt { .. }
        | Operation::LoadFsSelector
        | Operation::LoadGsSelector
        | Operation::LoadFsSelectorReg { .. }
        | Operation::LoadGsSelectorReg { .. }
        | Operation::LoadSegmentSelectorReg { .. }
        | Operation::LoadNullDsSelector
        | Operation::LoadNullEsSelector
        | Operation::LoadNullSsSelector
        | Operation::IretQ
        | Operation::Int3
        // ADR-0023 C1: LAR, LSL, VERR and the VERW forms of `DescriptorQuery` (descriptor
        // visibility DPL >= CPL and DPL >= RPL; Vol. 2A "LAR", Vol. 2B "LSL", "VERR/VERW").
        | Operation::DescriptorQuery { .. } => Cpl3Class::CplSpecific,
        // An honest `Unsupported` at CPL3 (ADR-0023, decision 1): far returns (`LRETQ`); SYSENTER, which
        // the set does not model from CPL3 (plan C1/C2a, question V12).
        Operation::Sysenter
        | Operation::FarReturn
        // ADR-0023 slice C2a (plan question V2, `c2a_cpl3_neutral_differential`, `every_neutral_variant_is_covered`):
        // `Neutral` by the SDM, but the set does not run them at CPL0 in any encoding on the inputs of the
        // test (no retire and no fault of the operation itself), so at CPL3 they are an honest stop too
        // ("operations of the narrow profiles", decision 1). The x87 forms of the bare run refuse the
        // represented FCS/FDS of the FP cell (only FNSTENV/FNSAVE/FRSTOR/FLDENV [RAX] and FNSTSW AX, with the
        // member forms, run in the set); twelve of them (F2XM1 ... FCOS, FBLD, FBSTP) give the #NM of the
        // bounded path at CPL0 with CR0.EM or CR0.TS = 1, `Unsupported` at CPL3 (ISSUE-729). The MMX
        // conversions `CVT*PI*`/`CVT*PI2*` refuse in the set because their executor `step_sse_mmx_convert`
        // needs `state.x87`, which the set keeps in the cell (`None`); the `67 F2 A6/A7` REPNE CMPS spellings
        // run only outside the set.
        | Operation::RepneCmpsbAddr32
        | Operation::RepneCmpsWdqAddr32 { .. }
        | Operation::Cvtpi2pdMemoryRax
        | Operation::Cvtpi2psMemoryRax
        | Operation::Cvtpi2pdXmm0Mm1
        | Operation::Cvtpi2psXmm0Mm1
        | Operation::Cvttps2piMm0Xmm1
        | Operation::Cvttps2piMm0MemoryRax
        | Operation::Cvttpd2piMm0Xmm1
        | Operation::Cvttpd2piMm0MemoryRax
        | Operation::Cvtpd2piMm0MemoryRax
        | Operation::Cvtps2piMm0Xmm1
        | Operation::Cvtps2piMm0MemoryRax
        | Operation::Cvtpd2piMm0Xmm1
        | Operation::FisttpMemory { .. }
        | Operation::Fnop
        | Operation::FnstcwM16Rax
        | Operation::FldcwM16Rax
        | Operation::FnstswM16Rax
        | Operation::FstpM64FpRax
        | Operation::FstpM80FpRax
        | Operation::FbldM80Rax
        | Operation::FbstpM80Rax
        | Operation::FldM80FpRax
        | Operation::FstM64FpRax
        | Operation::FldM64FpRax
        | Operation::Fchs
        | Operation::FxchSti { .. }
        | Operation::FldSti { .. }
        | Operation::Fincstp
        | Operation::Fdecstp
        | Operation::Fld1
        | Operation::Fldz
        | Operation::Fabs
        | Operation::Ftst
        | Operation::Fxam
        | Operation::Frndint
        | Operation::Fsqrt
        | Operation::Fprem
        | Operation::Fprem1
        | Operation::F2xm1
        | Operation::Fyl2x
        | Operation::Fptan
        | Operation::Fpatan
        | Operation::Fxtract
        | Operation::Fyl2xp1
        | Operation::Fsincos
        | Operation::Fscale
        | Operation::Fsin
        | Operation::Fcos
        | Operation::FstpM32FpRax
        | Operation::FldM32FpRax
        | Operation::FstM32FpRax
        | Operation::FistpM32IntRax
        | Operation::FistpM16IntRax
        | Operation::FistpM64IntRax
        | Operation::FistM16IntRax
        | Operation::FistM32IntRax
        | Operation::FildM16IntRax
        | Operation::FildM64IntRax
        | Operation::FcomSt { .. }
        | Operation::Fcomi { .. }
        | Operation::Fucomi { .. }
        | Operation::Fcomip { .. }
        | Operation::Fucomip { .. }
        | Operation::Fcmovb { .. }
        | Operation::Fcmovnb { .. }
        | Operation::Fcmovne { .. }
        | Operation::Fcmove { .. }
        | Operation::Fcmovbe { .. }
        | Operation::Fcmovnbe { .. }
        | Operation::Fcmovu { .. }
        | Operation::Fcmovnu { .. }
        | Operation::FcompSt { .. }
        | Operation::Fcompp
        | Operation::Fucompp
        | Operation::Fucom
        | Operation::Fucomp
        | Operation::FstpSti { .. }
        | Operation::FstSti { .. }
        | Operation::FmulSt0St { .. }
        | Operation::FdivSt0St { .. }
        | Operation::FdivrSt0St { .. }
        | Operation::FdivrStSt0 { .. }
        | Operation::FdivStSt0 { .. }
        | Operation::FdivpStSt0 { .. }
        | Operation::FdivrpStSt0 { .. }
        | Operation::FaddSt0St { .. }
        | Operation::FsubSt0St { .. }
        | Operation::FsubrSt0St { .. }
        | Operation::FsubStSt0 { .. }
        | Operation::FsubrStSt0 { .. }
        | Operation::FaddStSt0 { .. }
        | Operation::FmulStSt0 { .. }
        | Operation::FaddpStSt0 { .. }
        | Operation::FmulpStSt0 { .. }
        | Operation::FsubpStSt0 { .. }
        | Operation::FsubrpStSt0 { .. } => Cpl3Class::NotAtCpl3,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — integer, stack, near branches, strings, flags, fences, hints and NOPs (incl. CET-NOP: neutral while CR4.CET = 0,
        // which the CR4 policy keeps; `const _` with `CR4_CET` in `execute.rs` breaks the build when CET is admitted, and then
        // `CetNop`, `Endbr64Nop`, `RdsspqRax` move to `Cr4Gated`).
        Operation::Cpuid
        | Operation::ClearDirectionFlag
        | Operation::SetDirectionFlag
        | Operation::MovImmediate { .. }
        | Operation::Mov { .. }
        | Operation::MovSregStore { .. }
        | Operation::CarryFlag { .. }
        | Operation::Clflush { .. }
        | Operation::CetNop
        | Operation::Extend { .. }
        | Operation::Lea { .. }
        | Operation::Lahf
        | Operation::Sahf
        | Operation::Prefetchw
        | Operation::Mfence
        | Operation::Lfence
        | Operation::Sfence
        | Operation::Nop
        | Operation::StackRegister { .. }
        | Operation::PushFlags
        | Operation::Movs { .. }
        | Operation::RepMovs { .. }
        | Operation::Lods { .. }
        | Operation::Stos { .. }
        | Operation::Scas { .. }
        | Operation::Cmps { .. }
        | Operation::RepStos { .. }
        | Operation::RepLods { .. }
        | Operation::RepScas { .. }
        | Operation::RepneScas { .. }
        | Operation::RepneScasbAddr32
        | Operation::RepneScasWdqAddr32 { .. }
        | Operation::RepCmps { .. }
        | Operation::PushImmediate { .. }
        | Operation::PushMemory { .. }
        | Operation::PopMemory { .. }
        | Operation::SignExtendAccumulator { .. }
        | Operation::Bswap { .. }
        | Operation::Bit { .. }
        | Operation::BitScan { .. }
        | Operation::Arithmetic { .. }
        | Operation::Logic { .. }
        | Operation::Mul { .. }
        | Operation::Imul { .. }
        | Operation::ImulImmediate { .. }
        | Operation::Divide { .. }
        | Operation::Shift { .. }
        | Operation::Branch { .. }
        | Operation::Loop { .. }
        | Operation::Loope { .. }
        | Operation::Loopne { .. }
        | Operation::Jrcxz { .. }
        | Operation::SetCondition { .. }
        | Operation::ConditionalMove { .. }
        | Operation::Jump { .. }
        | Operation::Call { .. }
        | Operation::Return { .. }
        | Operation::Leave
        | Operation::Endbr64Nop
        | Operation::RdsspqRax
        | Operation::Pause
        | Operation::NopModrm
        | Operation::IndirectJump { .. }
        | Operation::IndirectCall { .. } => Cpl3Class::Neutral,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — the FS/GS forms (the segment base is a register of the state; CPL-independent).
        Operation::MovFsLoad { .. }
        | Operation::MovGsLoad { .. }
        | Operation::MovFsStore { .. }
        | Operation::MovGsStore { .. }
        | Operation::MovFsLoadQ { .. }
        | Operation::MovGsLoadQ { .. }
        | Operation::MovFsLoadD { .. }
        | Operation::MovGsLoadD { .. }
        | Operation::MovzxFsLoadW { .. }
        | Operation::MovzxGsLoadW { .. }
        | Operation::MovsxdFsLoad { .. }
        | Operation::MovsxdGsLoad { .. }
        | Operation::MovsxFsLoadB { .. }
        | Operation::MovsxGsLoadB { .. }
        | Operation::MovzxFsLoadB { .. }
        | Operation::MovzxGsLoadB { .. }
        | Operation::MovFsMemByteImm { .. }
        | Operation::MovGsMemByteImm { .. }
        | Operation::MovFsMemImm { .. }
        | Operation::MovGsMemImm { .. }
        | Operation::TestFsMemByte { .. }
        | Operation::TestGsMemByte { .. }
        | Operation::TestFsMemWord { .. }
        | Operation::TestGsMemWord { .. }
        | Operation::TestFsMemWide { .. }
        | Operation::TestGsMemWide { .. }
        | Operation::IncDecFsMem { .. }
        | Operation::IncDecGsMem { .. }
        | Operation::SubFsMem { .. }
        | Operation::SubGsMem { .. }
        | Operation::AddFsLoad { .. }
        | Operation::AddGsLoad { .. }
        | Operation::CmpFsLoad { .. }
        | Operation::CmpGsLoad { .. }
        | Operation::LogicFsLoad { .. }
        | Operation::LogicGsLoad { .. }
        | Operation::CmovFsLoad { .. }
        | Operation::CmovGsLoad { .. }
        | Operation::CmpFsMem { .. }
        | Operation::CmpGsMem { .. }
        | Operation::OrFsMem { .. }
        | Operation::OrGsMem { .. }
        | Operation::AddFsMem { .. }
        | Operation::AddGsMem { .. }
        | Operation::CmpFsMemImm { .. }
        | Operation::CmpGsMemImm { .. }
        | Operation::AddFsMemImm { .. }
        | Operation::AddGsMemImm { .. }
        | Operation::LogicFsMemImm { .. }
        | Operation::LogicGsMemImm { .. }
        | Operation::SubFsMemImm { .. }
        | Operation::SubGsMemImm { .. }
        | Operation::MovFsSelectorToRax
        | Operation::MovGsSelectorToRax
        | Operation::PushFsMem { .. }
        | Operation::PushGsMem { .. } => Cpl3Class::Neutral,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — the LOCK and exchange forms (split lock: decision 4).
        Operation::Xchg { .. }
        | Operation::XchgMemory { .. }
        | Operation::Xadd { .. }
        | Operation::XaddMemory { .. }
        | Operation::LockXaddMem { .. }
        | Operation::XaddDsMem { .. }
        | Operation::XaddFsMem { .. }
        | Operation::XaddGsMem { .. }
        | Operation::Cmpxchg { .. }
        | Operation::CmpxchgMemory { .. }
        | Operation::LockCmpxchgMem { .. }
        | Operation::CmpxchgFsMemByte { .. }
        | Operation::CmpxchgGsMemByte { .. }
        | Operation::CmpxchgFsMem { .. }
        | Operation::CmpxchgGsMem { .. }
        | Operation::Cmpxchg16b { .. }
        | Operation::Cmpxchg8b { .. }
        | Operation::LockIncMem { .. }
        | Operation::LockDecMem { .. }
        | Operation::LockOrMem { .. }
        | Operation::LockAddMem { .. }
        | Operation::LockSubMem { .. }
        | Operation::LockSubDwordImm8 { .. }
        | Operation::LockAddMemReg { .. }
        | Operation::LockSubMemReg { .. }
        | Operation::LockAndMem { .. }
        | Operation::LockXorMem { .. }
        | Operation::LockLogicMemReg { .. }
        | Operation::LockBitMem { .. }
        | Operation::LockBitMemImm { .. }
        | Operation::LockedRmw { .. } => Cpl3Class::Neutral,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — the FP state (FXSAVE/FXRSTOR, LDMXCSR/STMXCSR: decision 2).
        Operation::Stmxcsr(..)
        | Operation::FxsaveRipRelative(..)
        | Operation::Fxsave64RaxDisp8(..)
        | Operation::Fxsave64R12(..)
        | Operation::Fxrstor64RbxDisp8(..)
        | Operation::Fxrstor64RipRelative(..)
        | Operation::Fxrstor64Rdi(..)
        | Operation::Fxsave { .. }
        | Operation::Fxrstor { .. }
        | Operation::LdmxcsrMemory { .. } => Cpl3Class::Neutral,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — SSE..SSE4.2, POPCNT, CRC32.
        Operation::MovupsRegister { .. }
        | Operation::MovapsRegister { .. }
        | Operation::MovapsReverseRegister { .. }
        | Operation::MovapsStoreMemory { .. }
        | Operation::MovapsLoadMemory { .. }
        | Operation::MovdqaRegister { .. }
        | Operation::MovapdRegister { .. }
        | Operation::MovupdRegister { .. }
        | Operation::MovupdLoadMemory { .. }
        | Operation::MovupdStoreMemory { .. }
        | Operation::MovapdReverseRegister { .. }
        | Operation::MovupdReverseRegister { .. }
        | Operation::XorpsRegister { .. }
        | Operation::XorpdRegister { .. }
        | Operation::AndpdRegister { .. }
        | Operation::OrpdRegister { .. }
        | Operation::AndnpdRegister { .. }
        | Operation::PxorRegister { .. }
        | Operation::PandRegister { .. }
        | Operation::PandnRegister { .. }
        | Operation::PorRegister { .. }
        | Operation::PavgbRegister { .. }
        | Operation::PavgwRegister { .. }
        | Operation::PmaxubRegister { .. }
        | Operation::PminubRegister { .. }
        | Operation::PminubMemory { .. }
        | Operation::PmaxswRegister { .. }
        | Operation::PminswRegister { .. }
        | Operation::PmullwRegister { .. }
        | Operation::PmulhuwRegister { .. }
        | Operation::PsadbwRegister { .. }
        | Operation::PmovmskbXmmToGpr { .. }
        | Operation::PmuludqRegister { .. }
        | Operation::PmaddwdRegister { .. }
        | Operation::PaddbRegister { .. }
        | Operation::PaddwRegister { .. }
        | Operation::PadddRegister { .. }
        | Operation::PsubdRegister { .. }
        | Operation::PaddqRegister { .. }
        | Operation::PsubqRegister { .. }
        | Operation::PcmpeqbRegister { .. }
        | Operation::PcmpeqwRegister { .. }
        | Operation::PcmpeqdRegister { .. }
        | Operation::PcmpgtbRegister { .. }
        | Operation::PcmpgtwRegister { .. }
        | Operation::PcmpgtdRegister { .. }
        | Operation::PunpcklbwRegister { .. }
        | Operation::PunpckhbwRegister { .. }
        | Operation::PunpcklwdRegister { .. }
        | Operation::PunpckhwdRegister { .. }
        | Operation::PunpckldqRegister { .. }
        | Operation::PunpckhdqRegister { .. }
        | Operation::PunpcklqdqRegister { .. }
        | Operation::PunpckhqdqRegister { .. }
        | Operation::PsubwRegister { .. }
        | Operation::PadduswRegister { .. }
        | Operation::PsubuswRegister { .. }
        | Operation::PsubbRegister { .. }
        | Operation::PaddusbRegister { .. }
        | Operation::PaddsbRegister { .. }
        | Operation::PsubsbRegister { .. }
        | Operation::PaddswRegister { .. }
        | Operation::PsubswRegister { .. }
        | Operation::PacksswbRegister { .. }
        | Operation::PackuswbRegister { .. }
        | Operation::PackssdwRegister { .. }
        | Operation::PshufdRegister { .. }
        | Operation::PshuflwRegister { .. }
        | Operation::PinsrwMemory { .. }
        | Operation::PsllwImmediateRegister { .. }
        | Operation::PslldImmediateRegister { .. }
        | Operation::PsllqImmediateRegister { .. }
        | Operation::PslldqImmediateRegister { .. }
        | Operation::PsrldqImmediateRegister { .. }
        | Operation::PsrlqImmediateRegister { .. }
        | Operation::PsrldImmediateRegister { .. }
        | Operation::PsrlwImmediateRegister { .. }
        | Operation::PsrawImmediateRegister { .. }
        | Operation::PsradImmediateRegister { .. }
        | Operation::PsubusbRegister { .. }
        | Operation::AndpsRegister { .. }
        | Operation::OrpsRegister { .. }
        | Operation::AndnpsRegister { .. }
        | Operation::MovssRegister { .. }
        | Operation::RsqrtssRegister { .. }
        | Operation::RsqrtssMemoryRax
        | Operation::RcpssRegister { .. }
        | Operation::RsqrtpsRegister { .. }
        | Operation::RcppsRegister { .. }
        | Operation::RcppsMemoryRax
        | Operation::RcpssMemoryRax
        | Operation::RsqrtpsMemoryRax
        | Operation::MinssRegisterMasked { .. }
        | Operation::MinssMemoryRax
        | Operation::MaxssRegisterMasked { .. }
        | Operation::MaxssMemoryRax
        | Operation::MinsdRegisterMasked { .. }
        | Operation::MinsdMemoryRax
        | Operation::MaxsdRegisterMasked { .. }
        | Operation::MaxsdMemoryRax
        | Operation::MaxpdMemoryRax
        | Operation::MinpdMemoryRax
        | Operation::MaxpsMemoryRax
        | Operation::MinpdRegisterMasked { .. }
        | Operation::MaxpdRegisterMasked { .. }
        | Operation::Cvtdq2pdRegister { .. }
        | Operation::Cvtdq2pdMemoryRax
        | Operation::Cvtpd2dqRegister { .. }
        | Operation::Cvtpd2dqMemoryRax
        | Operation::Cvtpd2psRegister { .. }
        | Operation::Cvtpd2psMemoryRax
        | Operation::Cvttpd2dqRegister { .. }
        | Operation::SqrtssRegister { .. }
        | Operation::SqrtssMemoryRax
        | Operation::SqrtsdRegister { .. }
        | Operation::SqrtpsRegister { .. }
        | Operation::SqrtpdRegister { .. }
        | Operation::SubpdRegister { .. }
        | Operation::SubpdMemoryRax
        | Operation::MulpdMemoryRax
        | Operation::MulpdRegister { .. }
        | Operation::AddpdRegister { .. }
        | Operation::AddpdMemoryRax
        | Operation::AddsdRegister { .. }
        | Operation::AddsdMemoryRax
        | Operation::AddssRegister { .. }
        | Operation::AddssMemoryRax
        | Operation::AddpsRegister { .. }
        | Operation::AddpsMemoryRax
        | Operation::SubpsMemoryRax
        | Operation::DivpdRegister { .. }
        | Operation::DivpdMemoryRax
        | Operation::DivsdRegister { .. }
        | Operation::DivsdMemoryRax
        | Operation::DivpsRegister { .. }
        | Operation::DivpsMemoryRax
        | Operation::DivssRegister { .. }
        | Operation::DivssMemoryRax
        | Operation::MulsdRegister { .. }
        | Operation::MulssRegister { .. }
        | Operation::MulssMemoryRax
        | Operation::MulsdMemoryRax
        | Operation::MulsdMemory { .. }
        | Operation::MulpsRegister { .. }
        | Operation::MulpsMemoryRax
        | Operation::SubpsRegister { .. }
        | Operation::SubsdRegister { .. }
        | Operation::SubssRegister { .. }
        | Operation::SubsdMemoryRax
        | Operation::SubssMemoryRax
        | Operation::Cvtdq2psRegister { .. }
        | Operation::Cvtdq2psMemoryRax
        | Operation::Cvtps2dqRegister { .. }
        | Operation::Cvtps2dqMemoryRax
        | Operation::Cvttps2dqRegister { .. }
        | Operation::Cvttps2dqMemoryRax
        | Operation::Cvttpd2dqMemoryRax
        | Operation::Cvtss2sdRegister { .. }
        | Operation::Cvtss2sdMemoryRax
        | Operation::Cvtsd2ssRegister { .. }
        | Operation::Cvtsd2ssMemoryRax
        | Operation::Cvtps2pdRegister { .. }
        | Operation::Cvtps2pdMemoryRax
        | Operation::ComissUcomissRegister { .. }
        | Operation::ComisdMemoryRax
        | Operation::UcomisdMemoryRax
        | Operation::ComissMemoryRax
        | Operation::UcomissMemoryRax
        | Operation::CmpssRegister { .. }
        | Operation::CmpssMemoryRax { .. }
        | Operation::CmpsdMemoryRax { .. }
        | Operation::CmpsdRegister { .. }
        | Operation::CmppdRegister { .. }
        | Operation::CmppdMemoryRax { .. }
        | Operation::CmppsMemoryRax { .. }
        | Operation::CmppsRegister { .. }
        | Operation::Cvtsi2sdR32Register { .. }
        | Operation::Cvtsi2sdR32MemoryRax
        | Operation::Cvtsi2ssR32MemoryRax
        | Operation::Cvtsi2sdR64MemoryRax
        | Operation::Cvtsi2sdR64Register { .. }
        | Operation::Cvtsi2ssR32Register { .. }
        | Operation::Cvtsi2ssR64Register { .. }
        | Operation::Cvtsi2ssR64MemoryRax
        | Operation::Cvtss2siR32Register { .. }
        | Operation::Cvtss2siR32MemoryRax
        | Operation::Cvtss2siR64MemoryRax
        | Operation::Cvttss2siR32MemoryRax
        | Operation::Cvttss2siR64MemoryRax
        | Operation::Cvtss2siR64Register { .. }
        | Operation::Cvttss2siR32Register { .. }
        | Operation::Cvttss2siR64Register { .. }
        | Operation::Cvttsd2siR32Register { .. }
        | Operation::Cvttsd2siR32MemoryRax
        | Operation::Cvttsd2siR64Register { .. }
        | Operation::Cvttsd2siR64MemoryRax
        | Operation::Cvtsd2siR32Register { .. }
        | Operation::Cvtsd2siR32MemoryRax
        | Operation::Cvtsd2siR64MemoryRax
        | Operation::Cvtsd2siR64Register { .. }
        | Operation::MinpsRegisterMasked { .. }
        | Operation::MinpsMemoryRax
        | Operation::SqrtpsMemoryRax
        | Operation::SqrtpdMemoryRax
        | Operation::SqrtsdMemoryRax
        | Operation::MaxpsRegisterMasked { .. }
        | Operation::MovsdRegister { .. }
        | Operation::MovsdStoreRegister { .. }
        | Operation::MoveIntegerToXmm { .. }
        | Operation::MovqFromXmm { .. }
        | Operation::MovsdLoadMemory { .. }
        | Operation::MovsdStoreMemory { .. }
        | Operation::MovssLoadMemory { .. }
        | Operation::MovssStoreMemory { .. }
        | Operation::MovupsLoadMemory { .. }
        | Operation::MovdquLoadMemory { .. }
        | Operation::MovdqaLoadMemory { .. }
        | Operation::PcmpeqbMemory { .. }
        | Operation::MovlpdLoadMemory { .. }
        | Operation::MovhpdLoadMemory { .. }
        | Operation::MovhpsLoadMemory { .. }
        | Operation::MovupsStoreMemory { .. }
        | Operation::Sse128MemoryMove { .. }
        | Operation::Sse3Permute { .. }
        | Operation::Ssse3Int { .. }
        | Operation::Sse41 { .. }
        | Operation::Sse2Int { .. }
        | Operation::Sse2Maskmov { .. }
        | Operation::Sse2MmxMove { .. }
        | Operation::SseFpArith { .. }
        | Operation::SseFpMisc { .. }
        | Operation::SseCvtFromInt { .. }
        | Operation::SseCvtToInt { .. }
        | Operation::Sse41Fp { .. }
        | Operation::SseLogicPermute { .. }
        | Operation::SseMovmsk { .. }
        | Operation::Popcnt { .. }
        | Operation::Crc32 { .. } => Cpl3Class::Neutral,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — MMX.
        Operation::MmxSsse3 { .. }
        | Operation::Emms
        | Operation::MmxInt { .. }
        | Operation::MmxShiftImmediate { .. }
        | Operation::MmxMovd { .. }
        | Operation::MmxMovq { .. } => Cpl3Class::Neutral,
        // Neutral (Vol. 3A §5.6.1: explicit accesses with the privilege of the context, implicit
        // descriptor-table accesses as supervisor) — x87.
        Operation::Fninit
        | Operation::Fwait
        | Operation::Fnclex
        | Operation::FnstswAx
        | Operation::FnstswM16 { .. }
        | Operation::FnstcwM16 { .. }
        | Operation::FfreeSti { .. }
        | Operation::FnstenvRax
        | Operation::FnsaveRax
        | Operation::FrstorRax
        | Operation::FldenvRax
        | Operation::FildM32IntRax
        | Operation::FildM32Int { .. }
        | Operation::FnsaveMemory { .. } => Cpl3Class::Neutral,
    }
}

/// The decision of the privilege layer: `class` at `cpl` with `iopl` and `cr4` (ADR-0023, decision 1).
/// Only CR4.TSD, CR4.PCE and CR4.UMIP are read; the CR4 policy of the set keeps TSD and UMIP clear and
/// PCE clear too (`const _` checks next to `CR4_TSD` in `execute.rs`), so at CPL3 `RDTSC` executes,
/// `RDPMC` is #GP(0) and `SGDT`/`SIDT`/`SLDT`/`STR` execute.
pub const fn cpl3_gate(class: Cpl3Class, cr4: u64, cpl: u8, iopl: u8) -> Cpl3Gate {
    match class {
        Cpl3Class::Neutral => Cpl3Gate::Execute,
        Cpl3Class::Privileged => gp0_if(cpl > 0),
        Cpl3Class::Cr4Gated(Cr4Gate::Tsd) => gp0_if(cpl > 0 && cr4 & CR4_TSD != 0),
        Cpl3Class::Cr4Gated(Cr4Gate::Pce) => gp0_if(cpl > 0 && cr4 & CR4_PCE == 0),
        Cpl3Class::Cr4Gated(Cr4Gate::Umip) => gp0_if(cpl > 0 && cr4 & CR4_UMIP != 0),
        // CR4.PVI is outside the policy of the set (`CliPreconditionFailed`): no virtual IF.
        Cpl3Class::IoplSensitive(IoplKind::InterruptFlag) => gp0_if(cpl > iopl),
        // At CPL > IOPL the executor reads the bit map of the TSS (its #PF precedes the #GP(0));
        // at CPL <= IOPL the access is allowed, and the CPL3 entry has no port (NotAtCpl3).
        Cpl3Class::IoplSensitive(IoplKind::PortIo) => {
            if cpl > iopl {
                Cpl3Gate::ByExecutor
            } else if cpl == 0 {
                Cpl3Gate::Execute
            } else {
                Cpl3Gate::NotModeled
            }
        }
        Cpl3Class::IoplSensitive(IoplKind::Popf) | Cpl3Class::CplSpecific => Cpl3Gate::ByExecutor,
        Cpl3Class::NotAtCpl3 => {
            if cpl == 3 {
                Cpl3Gate::NotModeled
            } else {
                Cpl3Gate::Execute
            }
        }
    }
}

const fn gp0_if(condition: bool) -> Cpl3Gate {
    if condition {
        Cpl3Gate::Gp0
    } else {
        Cpl3Gate::Execute
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::long_decode;
    use crate::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

    const CR4_FSGSBASE: u64 = 1 << 16;
    const CR4_OSXSAVE: u64 = 1 << 18;
    const CR4_PKE: u64 = 1 << 22;
    const CR4_CET: u64 = 1 << 23;

    /// Every class and kind, with its literal table: for the value 0 and 1 of the CR4 bit the kind
    /// reads (TSD, PCE or UMIP; the others read none), four rows CPL 0..3 of four letters IOPL 0..3.
    /// E = Execute, G = Gp0, B = ByExecutor, N = NotModeled. Written from ADR-0023 decision 1 and the
    /// SDM pages, not from `cpl3_gate`.
    const TABLE: [(Cpl3Class, u64, [&str; 4], [&str; 4]); 10] = {
        const ALL_E: [&str; 4] = ["EEEE", "EEEE", "EEEE", "EEEE"];
        const GP_ABOVE_0: [&str; 4] = ["EEEE", "GGGG", "GGGG", "GGGG"];
        const ALL_B: [&str; 4] = ["BBBB", "BBBB", "BBBB", "BBBB"];
        [
            (Cpl3Class::Neutral, 0, ALL_E, ALL_E),
            (Cpl3Class::Privileged, 0, GP_ABOVE_0, GP_ABOVE_0),
            (Cpl3Class::Cr4Gated(Cr4Gate::Tsd), 1 << 2, ALL_E, GP_ABOVE_0),
            (Cpl3Class::Cr4Gated(Cr4Gate::Pce), 1 << 8, GP_ABOVE_0, ALL_E),
            (
                Cpl3Class::Cr4Gated(Cr4Gate::Umip),
                1 << 11,
                ALL_E,
                GP_ABOVE_0,
            ),
            (
                Cpl3Class::IoplSensitive(IoplKind::InterruptFlag),
                0,
                ["EEEE", "GEEE", "GGEE", "GGGE"],
                ["EEEE", "GEEE", "GGEE", "GGGE"],
            ),
            (
                Cpl3Class::IoplSensitive(IoplKind::PortIo),
                0,
                ["EEEE", "BNNN", "BBNN", "BBBN"],
                ["EEEE", "BNNN", "BBNN", "BBBN"],
            ),
            (Cpl3Class::IoplSensitive(IoplKind::Popf), 0, ALL_B, ALL_B),
            (Cpl3Class::CplSpecific, 0, ALL_B, ALL_B),
            (
                Cpl3Class::NotAtCpl3,
                0,
                ["EEEE", "EEEE", "EEEE", "NNNN"],
                ["EEEE", "EEEE", "EEEE", "NNNN"],
            ),
        ]
    };

    fn letter(gate: Cpl3Gate) -> char {
        match gate {
            Cpl3Gate::Execute => 'E',
            Cpl3Gate::Gp0 => 'G',
            Cpl3Gate::ByExecutor => 'B',
            Cpl3Gate::NotModeled => 'N',
        }
    }

    /// The eight combinations of TSD, PCE and UMIP over a base CR4 (PAE and the bits of the policy).
    fn gate_bits() -> impl Iterator<Item = u64> {
        (0..8u64).map(|combination| {
            0x6f8
                | if combination & 1 != 0 { 1 << 2 } else { 0 }
                | if combination & 2 != 0 { 1 << 8 } else { 0 }
                | if combination & 4 != 0 { 1 << 11 } else { 0 }
        })
    }

    #[test]
    fn gate_table_matches_an_independent_table_for_every_cr4_bit_cpl_and_iopl() {
        let mut checked = 0;
        for (class, bit, clear, set) in TABLE {
            for cr4 in gate_bits() {
                let rows = if bit != 0 && cr4 & bit != 0 {
                    set
                } else {
                    clear
                };
                for cpl in 0..4u8 {
                    for iopl in 0..4u8 {
                        let expected = rows[usize::from(cpl)].as_bytes()[usize::from(iopl)] as char;
                        assert_eq!(
                            letter(cpl3_gate(class, cr4, cpl, iopl)),
                            expected,
                            "{class:?} CR4 {cr4:#x} CPL {cpl} IOPL {iopl}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 10 * 8 * 16);
    }

    #[test]
    fn bits_outside_the_policy_do_not_change_the_gate_of_other_classes() {
        for (class, ..) in TABLE {
            for cr4 in gate_bits() {
                for extra in [CR4_FSGSBASE, CR4_OSXSAVE, CR4_PKE, CR4_CET] {
                    for cpl in 0..4u8 {
                        for iopl in 0..4u8 {
                            assert_eq!(
                                cpl3_gate(class, cr4 | extra, cpl, iopl),
                                cpl3_gate(class, cr4, cpl, iopl),
                                "{class:?} CR4 {cr4:#x} + {extra:#x} CPL {cpl} IOPL {iopl}"
                            );
                        }
                    }
                }
            }
        }
    }

    struct Bytes<'a>(&'a [u8]);

    impl GuestMemory for Bytes<'_> {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.0
                .get(address as usize)
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
            _: &mut [u8],
        ) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
        fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[test]
    fn named_members_have_their_adr_class() {
        use Cpl3Class::{CplSpecific, Neutral, NotAtCpl3, Privileged};
        const UMIP: Cpl3Class = Cpl3Class::Cr4Gated(Cr4Gate::Umip);
        const IF: Cpl3Class = Cpl3Class::IoplSensitive(IoplKind::InterruptFlag);
        const PIO: Cpl3Class = Cpl3Class::IoplSensitive(IoplKind::PortIo);
        // (bytes, the variant they decode to, the class of ADR-0023 decision 1)
        let table: &[(&[u8], &str, Cpl3Class)] = &[
            (&[0xf4], "Hlt", Privileged),
            (&[0x0f, 0x01, 0x38], "Invlpg", Privileged),
            (&[0x0f, 0x01, 0xf8], "Swapgs", Privileged),
            (&[0x48, 0x0f, 0x07], "Sysret", Privileged),
            (&[0x0f, 0x35], "Sysexit", Privileged),
            (&[0x0f, 0x09], "Wbinvd", Privileged),
            (&[0x0f, 0x32], "ReadMsr", Privileged),
            (&[0x0f, 0x30], "WriteMsr", Privileged),
            (&[0x0f, 0x01, 0x10], "LoadGdtr", Privileged),
            (&[0x0f, 0x01, 0x18], "LoadIdtr", Privileged),
            (&[0x0f, 0x00, 0xd0], "LoadLdt", Privileged),
            (&[0x0f, 0x00, 0xd8], "LoadTr", Privileged),
            (&[0x0f, 0x20, 0xc0], "ReadCr0", Privileged),
            (&[0x0f, 0x22, 0xc0], "WriteCr0", Privileged),
            (&[0x0f, 0x22, 0xd8], "WriteCr3", Privileged),
            (&[0x0f, 0x21, 0xf8], "ReadDebugRegister", Privileged),
            (&[0x0f, 0x23, 0xf8], "WriteDebugRegister", Privileged),
            (&[0x0f, 0x31], "Rdtsc", Cpl3Class::Cr4Gated(Cr4Gate::Tsd)),
            (&[0x0f, 0x33], "Rdpmc", Cpl3Class::Cr4Gated(Cr4Gate::Pce)),
            (&[0x0f, 0x01, 0x00], "StoreGdtr", UMIP),
            (&[0x0f, 0x01, 0x08], "StoreIdtr", UMIP),
            (&[0x0f, 0x00, 0x00], "StoreSystemSelector", UMIP),
            (&[0xfa], "Cli", IF),
            (&[0xfb], "Sti", IF),
            (&[0xec], "InAlDx", PIO),
            (&[0xee], "OutDxAl", PIO),
            (&[0xe4, 0x60], "InAlImm8", PIO),
            (
                &[0x9d],
                "PopFlags",
                Cpl3Class::IoplSensitive(IoplKind::Popf),
            ),
            (&[0x48, 0xcf], "IretQ", CplSpecific),
            (&[0xcd, 0x2e], "SoftwareInterrupt", CplSpecific),
            (&[0xcc], "Int3", CplSpecific),
            (&[0x0f, 0x05], "Syscall", CplSpecific),
            (&[0x8e, 0x18], "MovToSreg", CplSpecific),
            (&[0x0f, 0x00, 0x28], "VerwMem", CplSpecific),
            (&[0x48, 0xcb], "FarReturn", NotAtCpl3),
            (&[0x0f, 0x34], "Sysenter", NotAtCpl3),
            (&[0x0f, 0xa2], "Cpuid", Neutral),
            (&[0x0f, 0xae, 0x00], "Fxsave", Neutral),
            (&[0x0f, 0xae, 0x38], "Clflush", Neutral),
            (&[0x0f, 0xae, 0xe8], "Lfence", Neutral),
            (&[0xf3, 0x90], "Pause", Neutral),
            (&[0xf3, 0x0f, 0x1e, 0xfa], "Endbr64Nop", Neutral),
            (&[0xf3, 0x48, 0x0f, 0x1e, 0xc8], "RdsspqRax", Neutral),
            (&[0x8c, 0xd8], "MovSregStore", Neutral),
            (&[0x9c], "PushFlags", Neutral),
            (&[0x48, 0x89, 0xc8], "Mov", Neutral),
        ];
        for (bytes, variant, class) in table {
            let instruction = long_decode::decode(&mut Bytes(bytes), 0, AccessContext::default())
                .unwrap_or_else(|error| panic!("{bytes:02x?}: {error:?}"));
            let name = format!("{:?}", instruction.operation);
            assert!(
                name == *variant
                    || name.starts_with(&format!("{variant} "))
                    || name.starts_with(&format!("{variant}(")),
                "{bytes:02x?} decodes to {name}, not {variant}"
            );
            assert_eq!(cpl3_class(&instruction.operation), *class, "{name}");
        }
    }
}
